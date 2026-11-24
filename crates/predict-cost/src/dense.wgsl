@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read> parameters: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<f32>;
var<workgroup> sums: array<f32, LANES>;

@compute @workgroup_size(LANES)
fn dense(@builtin(workgroup_id) group: vec3<u32>,
         @builtin(local_invocation_index) lane: u32) {
    let row = group.x;
    var sum = 0.0;
    for (var col = lane; col < INPUTS; col += LANES) {
        sum += parameters[row * INPUTS + col] * input[col];
    }
    sums[lane] = sum;
    for (var stride = LANES / 2u; stride > 0u; stride /= 2u) {
        workgroupBarrier();
        if lane < stride {
            sums[lane] += sums[lane + stride];
        }
    }
    if lane == 0u {
        let value = sums[0] + parameters[INPUTS * OUTPUTS + row];
        output[row] = select(value, max(0.0, value), RELU);
    }
}
