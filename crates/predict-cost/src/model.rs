pub const INPUTS: usize = 64;
pub const OUTPUTS: usize = 4;
pub const EPSILON: f32 = 1e-3;
pub type Features = [f32; INPUTS];
pub type SyntheticOutput = [f32; OUTPUTS];

pub struct SyntheticMlpCostModel {
    pub layers: [Layer; 3],
}

impl SyntheticMlpCostModel {
    pub fn new(width: usize, seed: u64) -> Self {
        let mut random = Random(seed);
        Self {
            layers: [
                Layer::new(INPUTS, width, true, &mut random),
                Layer::new(width, width, true, &mut random),
                Layer::new(width, OUTPUTS, false, &mut random),
            ],
        }
    }

    pub fn parameters(&self) -> usize {
        self.layers.iter().map(|layer| layer.parameters.len()).sum()
    }
}

pub struct Layer {
    pub inputs: usize,
    pub outputs: usize,
    pub relu: bool,
    // An output-major weight matrix followed by one bias per output.
    pub parameters: Vec<f32>,
}

impl Layer {
    fn new(inputs: usize, outputs: usize, relu: bool, random: &mut Random) -> Self {
        let scale = (3.0 / inputs as f32).sqrt();
        let mut parameters: Vec<_> = (0..inputs * outputs)
            .map(|_| random.symmetric() * scale)
            .collect();
        parameters.extend((0..outputs).map(|_| random.symmetric() * 0.05));
        Self {
            inputs,
            outputs,
            relu,
            parameters,
        }
    }

    pub fn forward_rows(&self, input: &[f32], start_row: usize, output: &mut [f32]) {
        debug_assert_eq!(input.len(), self.inputs);
        let first = start_row * self.inputs;
        let last = first + output.len() * self.inputs;
        let biases = self.parameters[self.inputs * self.outputs + start_row..].iter();
        for ((row, &bias), value) in self.parameters[first..last]
            .chunks_exact(self.inputs)
            .zip(biases)
            .zip(output)
        {
            let mut sum = bias;
            for (&weight, &feature) in row.iter().zip(input) {
                sum += weight * feature;
            }
            *value = if self.relu { sum.max(0.0) } else { sum };
        }
    }
}

pub fn features(seed: u64) -> [Features; 8] {
    let mut random = Random(seed ^ 0x5ea2_ab51_9bd3_4c37);
    std::array::from_fn(|_| std::array::from_fn(|_| random.symmetric()))
}

struct Random(u64);

impl Random {
    fn symmetric(&mut self) -> f32 {
        // SplitMix64. The high 24 bits are exactly representable in f32.
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^= value >> 31;
        (value >> 40) as f32 * (2.0 / 16_777_216.0) - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dense_relu_and_bias() {
        let mut layer = Layer {
            inputs: 2,
            outputs: 2,
            relu: true,
            parameters: vec![1.0, 2.0, -2.0, 5.0, 3.0, -1.0],
        };
        let mut output = [0.0; 2];
        layer.forward_rows(&[2.0, 1.0], 0, &mut output);
        assert_eq!(output, [7.0, 0.0]);
        layer.relu = false;
        layer.forward_rows(&[2.0, 1.0], 1, &mut output[1..]);
        assert_eq!(output[1], 0.0);
        layer.forward_rows(&[3.0, 1.0], 1, &mut output[1..]);
        assert_eq!(output[1], -2.0);
    }

    #[test]
    fn weights_and_features_are_seed_deterministic() {
        let (a, b, c) = (
            SyntheticMlpCostModel::new(71, 42),
            SyntheticMlpCostModel::new(71, 42),
            SyntheticMlpCostModel::new(71, 43),
        );
        for ((a, b), c) in a.layers.iter().zip(&b.layers).zip(&c.layers) {
            assert_eq!(a.parameters, b.parameters);
            assert_ne!(a.parameters, c.parameters);
        }
        assert_eq!(features(42), features(42));
        assert_ne!(features(42), features(43));
    }
}
