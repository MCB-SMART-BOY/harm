
@group(0) @binding(0) var<storage, read> left: array<f32>;
@group(0) @binding(1) var<storage, read> right: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<f32>;
var<workgroup> left_tile: array<array<f32, 16>, 16>;
var<workgroup> right_tile: array<array<f32, 16>, 16>;

@compute @workgroup_size(16, 16)
fn matmul(@builtin(global_invocation_id) global: vec3<u32>,
          @builtin(local_invocation_id) local: vec3<u32>) {
    let row = global.y;
    let column = global.x;
    var sum = 0.0;
    for (var base = 0u; base < K; base += 16u) {
        let left_depth = base + local.x;
        let right_depth = base + local.y;
        var a = 0.0;
        var b = 0.0;
        if (row < M && left_depth < K) {
            if (TRANSPOSE_LEFT) {
                a = left[left_depth * M + row];
            } else {
                a = left[row * K + left_depth];
            }
        }
        if (column < N && right_depth < K) {
            b = right[right_depth * N + column];
        }
        left_tile[local.y][local.x] = a;
        right_tile[local.y][local.x] = b;
        workgroupBarrier();
        for (var depth = 0u; depth < 16u; depth++) {
            sum += left_tile[local.y][depth] * right_tile[depth][local.x];
        }
        workgroupBarrier();
    }
    if (row < M && column < N) {
        output[row * N + column] = sum;
    }
}
