
@group(0) @binding(0) var<storage, read> first: array<f32>;
@group(0) @binding(1) var<storage, read_write> second: array<f32>;
@group(0) @binding(2) var<storage, read_write> third: array<f32>;
var<workgroup> scratch: array<f32, 256>;

@compute @workgroup_size(256)
fn residual(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < M * N) {
        third[id.x] = first[id.x] - second[id.x];
    }
}

@compute @workgroup_size(256)
fn update(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < K * N) {
        second[id.x] -= LR * (first[id.x] / f32(M));
    }
}

fn reduce(i: u32) {
    workgroupBarrier();
    for (var stride = 128u; stride > 0u; stride /= 2u) {
        if (i < stride) {
            scratch[i] += scratch[i + stride];
        }
        workgroupBarrier();
    }
}

@compute @workgroup_size(256)
fn partial_loss(@builtin(global_invocation_id) id: vec3<u32>,
                @builtin(local_invocation_index) i: u32,
                @builtin(workgroup_id) group: vec3<u32>) {
    var difference = 0.0;
    if (id.x < M * N) {
        difference = first[id.x] - second[id.x];
    }
    scratch[i] = difference * difference;
    reduce(i);
    if (i == 0u) {
        third[group.x] = scratch[0];
    }
}

@compute @workgroup_size(256)
fn loss(@builtin(local_invocation_index) i: u32) {
    var sum = 0.0;
    for (var j = i; j < (M * N + 255u) / 256u; j += 256u) {
        sum += first[j];
    }
    scratch[i] = sum;
    reduce(i);
    if (i == 0u) {
        second[0] = scratch[0] / (2.0 * f32(M));
    }
}
