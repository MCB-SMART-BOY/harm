use super::{EPSILON, cpu_matmul, max_abs_error, open_device, vulkan_adapters};
use bytemuck::cast_slice;
use std::{fmt, sync::mpsc, time::Duration};
use wgpu::util::DeviceExt;

#[derive(Debug)]
pub enum GpuError {
    NoHardwareVulkan,
    Runtime(String),
    WrongOutput { expected: Option<f32>, actual: f32 },
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHardwareVulkan => {
                write!(f, "no hardware Vulkan adapter (software adapters excluded)")
            }
            Self::Runtime(message) => f.write_str(message),
            Self::WrongOutput { expected, actual } => {
                write!(
                    f,
                    "GPU validation failed (expected={expected:?}, actual={actual}, tolerance={EPSILON})"
                )
            }
        }
    }
}

pub struct Compute {
    info: wgpu::AdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl Compute {
    pub fn open() -> Result<Self, GpuError> {
        let adapters = vulkan_adapters();
        let adapter = adapters
            .iter()
            .filter(|a| {
                matches!(
                    a.get_info().device_type,
                    wgpu::DeviceType::DiscreteGpu
                        | wgpu::DeviceType::IntegratedGpu
                        | wgpu::DeviceType::VirtualGpu
                )
            })
            .min_by_key(|a| a.get_info().device_type != wgpu::DeviceType::DiscreteGpu)
            .ok_or(GpuError::NoHardwareVulkan)?;
        let info = adapter.get_info();
        let (device, queue) = open_device(adapter).map_err(GpuError::Runtime)?;
        Ok(Self {
            info,
            device,
            queue,
        })
    }

    pub fn adapter(&self) -> String {
        format!("{:?}", self.info)
    }

    fn pipeline(&self, source: &str, entry: &str, buffers: &[&wgpu::Buffer]) -> Dispatch {
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(entry),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            });
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(binding, buffer)| wgpu::BindGroupEntry {
                binding: binding as u32,
                resource: buffer.as_entire_binding(),
            })
            .collect();
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(entry),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        Dispatch { pipeline, bindings }
    }

    fn storage(&self, values: &[f32]) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: cast_slice(values),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            })
    }

    fn buffer(&self, count: usize, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (count * size_of::<f32>()) as u64,
            usage,
            mapped_at_creation: false,
        })
    }

    fn readback(&self, count: usize) -> wgpu::Buffer {
        self.buffer(
            count,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        )
    }

    fn encoder(&self) -> wgpu::CommandEncoder {
        self.device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default())
    }

    fn submit_read<T>(
        &self,
        mut encoder: wgpu::CommandEncoder,
        output: &wgpu::Buffer,
        readback: &wgpu::Buffer,
        check: impl FnOnce(&[f32]) -> Result<T, GpuError>,
    ) -> Result<T, GpuError> {
        encoder.copy_buffer_to_buffer(output, 0, readback, 0, readback.size());
        let index = self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: Some(Duration::from_secs(10)),
            })
            .map_err(|e| GpuError::Runtime(format!("GPU poll: {e}")))?;
        receiver
            .try_recv()
            .map_err(|e| GpuError::Runtime(format!("GPU map callback: {e}")))?
            .map_err(|e| GpuError::Runtime(format!("GPU map: {e}")))?;
        let mapped = slice.get_mapped_range();
        let result = check(cast_slice(&mapped));
        drop(mapped);
        readback.unmap();
        result
    }
}

struct Dispatch {
    pipeline: wgpu::ComputePipeline,
    bindings: wgpu::BindGroup,
}

impl Dispatch {
    fn encode(&self, encoder: &mut wgpu::CommandEncoder, x: u32, y: u32, count: u32) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bindings, &[]);
        for _ in 0..count {
            pass.dispatch_workgroups(x, y, 1);
        }
    }
}

fn matmul(
    compute: &Compute,
    [m, k, n]: [u32; 3],
    transpose_left: bool,
    buffers: [&wgpu::Buffer; 3],
) -> Dispatch {
    let constants = format!(
        "const M = {m}u; const K = {k}u; const N = {n}u; const TRANSPOSE_LEFT = {transpose_left};"
    );
    compute.pipeline(
        &(constants + include_str!("matrix-matmul.wgsl")),
        "matmul",
        &buffers,
    )
}

pub struct Inference {
    compute: Compute,
    dispatch: Dispatch,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    expected: Vec<f32>,
    grid: [u32; 2],
    dispatches: u32,
}

impl Inference {
    pub fn new(compute: Compute, shape: [u32; 3], x: &[f32], w: &[f32], dispatches: u32) -> Self {
        let [m, k, n] = shape;
        let size = (m * n) as usize;
        let output = compute.buffer(
            size,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let readback = compute.readback(size);
        let dispatch = matmul(
            &compute,
            shape,
            false,
            [&compute.storage(x), &compute.storage(w), &output],
        );
        let expected = cpu_matmul(x, w, m as usize, k as usize, n as usize);
        Self {
            compute,
            dispatch,
            output,
            readback,
            expected,
            grid: [n.div_ceil(16), m.div_ceil(16)],
            dispatches,
        }
    }

    pub fn batch(&self) -> Result<f32, GpuError> {
        let mut encoder = self.compute.encoder();
        self.dispatch
            .encode(&mut encoder, self.grid[0], self.grid[1], self.dispatches);
        self.compute
            .submit_read(encoder, &self.output, &self.readback, |values| {
                let error = max_abs_error(&self.expected, values);
                if error <= EPSILON {
                    Ok(error)
                } else {
                    Err(GpuError::WrongOutput {
                        expected: Some(0.0),
                        actual: error,
                    })
                }
            })
    }
}

pub struct Training {
    compute: Compute,
    weights: wgpu::Buffer,
    forward: Dispatch,
    residual: Dispatch,
    gradient: Dispatch,
    update: Dispatch,
    partial_loss: Dispatch,
    loss: Dispatch,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    shape: [u32; 3],
}

impl Training {
    pub fn new(
        compute: Compute,
        shape: [u32; 3],
        x: &[f32],
        w: &[f32],
        y: &[f32],
        lr: f32,
    ) -> Self {
        let [m, k, n] = shape;
        let inputs = compute.storage(x);
        let weights = compute.storage(w);
        let targets = compute.storage(y);
        let storage = |count| compute.buffer(count as usize, wgpu::BufferUsages::STORAGE);
        let prediction = storage(m * n);
        let residuals = storage(m * n);
        let gradients = storage(k * n);
        let partial = storage((m * n).div_ceil(256));
        let output = compute.buffer(
            1,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let readback = compute.readback(1);
        let constants =
            format!("const M = {m}u; const K = {k}u; const N = {n}u; const LR = {lr:?};");
        let shader = constants + include_str!("matrix-training.wgsl");
        let forward = matmul(&compute, shape, false, [&inputs, &weights, &prediction]);
        let gradient = matmul(&compute, [k, m, n], true, [&inputs, &residuals, &gradients]);
        let residual = compute.pipeline(&shader, "residual", &[&prediction, &targets, &residuals]);
        let update = compute.pipeline(&shader, "update", &[&gradients, &weights]);
        let partial_loss =
            compute.pipeline(&shader, "partial_loss", &[&prediction, &targets, &partial]);
        let loss = compute.pipeline(&shader, "loss", &[&partial, &output]);
        Self {
            compute,
            weights,
            forward,
            residual,
            gradient,
            update,
            partial_loss,
            loss,
            output,
            readback,
            shape,
        }
    }

    pub fn reset(&self, initial: &[f32]) -> Result<(), GpuError> {
        self.compute
            .queue
            .write_buffer(&self.weights, 0, cast_slice(initial));
        let index = self.compute.queue.submit([]);
        self.compute
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: Some(Duration::from_secs(10)),
            })
            .map_err(|e| GpuError::Runtime(format!("GPU restore input buffers: {e}")))?;
        Ok(())
    }

    pub fn step(&self) -> Result<f32, GpuError> {
        let [m, k, n] = self.shape;
        let mut encoder = self.compute.encoder();
        self.forward
            .encode(&mut encoder, n.div_ceil(16), m.div_ceil(16), 1);
        self.residual
            .encode(&mut encoder, (m * n).div_ceil(256), 1, 1);
        self.gradient
            .encode(&mut encoder, n.div_ceil(16), k.div_ceil(16), 1);
        self.update
            .encode(&mut encoder, (k * n).div_ceil(256), 1, 1);
        self.forward
            .encode(&mut encoder, n.div_ceil(16), m.div_ceil(16), 1);
        self.partial_loss
            .encode(&mut encoder, (m * n).div_ceil(256), 1, 1);
        self.loss.encode(&mut encoder, 1, 1, 1);
        self.compute
            .submit_read(encoder, &self.output, &self.readback, |values| {
                let actual = values[0];
                if actual.is_finite() {
                    Ok(actual)
                } else {
                    Err(GpuError::WrongOutput {
                        expected: None,
                        actual,
                    })
                }
            })
    }

    pub fn check_first_step(
        &self,
        x: &[f32],
        w: &[f32],
        y: &[f32],
        lr: f32,
    ) -> Result<f32, GpuError> {
        let [m, k, n] = self.shape.map(|v| v as usize);
        let mut residual = cpu_matmul(x, w, m, k, n);
        for (r, &target) in residual.iter_mut().zip(y) {
            *r -= target;
        }
        let mut transpose = vec![0.0; x.len()];
        for row in 0..m {
            for column in 0..k {
                transpose[column * m + row] = x[row * k + column];
            }
        }
        let gradient = cpu_matmul(&transpose, &residual, k, m, n);
        let updated: Vec<_> = w
            .iter()
            .zip(gradient)
            .map(|(&weight, g)| weight - lr * (g / m as f32))
            .collect();
        let expected = cpu_matmul(x, &updated, m, k, n)
            .iter()
            .zip(y)
            .map(|(&prediction, &target)| (prediction - target).powi(2))
            .sum::<f32>()
            / (2 * m) as f32;
        let actual = self.step()?;
        if max_abs_error(&[expected], &[actual]) <= EPSILON {
            Ok(actual)
        } else {
            Err(GpuError::WrongOutput {
                expected: Some(expected),
                actual,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hardware() -> Option<Compute> {
        match Compute::open() {
            Ok(compute) => Some(compute),
            Err(GpuError::NoHardwareVulkan) => {
                eprintln!("SKIP matrix GPU test: no hardware Vulkan adapter");
                None
            }
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn inference_verifies_checked_readback() {
        let Some(compute) = hardware() else { return };
        let input: Vec<_> = (0..128 * 128)
            .map(|i| (i % 101) as f32 * 0.0005 - 0.025)
            .collect();
        let inference = Inference::new(compute, [128; 3], &input, &input, 32);
        assert!(inference.batch().unwrap() <= EPSILON);
    }

    #[test]
    fn training_checks_post_update_loss_and_reset() {
        let Some(compute) = hardware() else { return };
        let input: Vec<_> = (0..512 * 512)
            .map(|i| (i % 101) as f32 * 0.0005 - 0.025)
            .collect();
        let weights: Vec<_> = (0..512 * 512)
            .map(|i| (i % 89) as f32 * 0.0005 - 0.022)
            .collect();
        let training = Training::new(compute, [512; 3], &input, &weights, &input, 0.001);
        let first = training
            .check_first_step(&input, &weights, &input, 0.001)
            .unwrap();
        assert!(training.step().unwrap().is_finite());
        training.reset(&weights).unwrap();
        assert_eq!(first, training.step().unwrap());
    }
}
