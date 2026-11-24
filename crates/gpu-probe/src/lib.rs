pub const EPSILON: f32 = 1.0e-3;

pub fn vulkan_adapters() -> Vec<wgpu::Adapter> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });
    instance.enumerate_adapters(wgpu::Backends::VULKAN)
}

pub fn open_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), String> {
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("HARM Vulkan compute device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|error| format!("wgpu request_device failed: {error}"))
}

pub fn cpu_matmul(
    left: &[f32],
    right: &[f32],
    rows: usize,
    inner: usize,
    columns: usize,
) -> Vec<f32> {
    let mut output = vec![0.0; rows * columns];
    for row in 0..rows {
        for depth in 0..inner {
            let value = left[row * inner + depth];
            for column in 0..columns {
                output[row * columns + column] += value * right[depth * columns + column];
            }
        }
    }
    output
}

pub fn max_abs_error(expected: &[f32], actual: &[f32]) -> f32 {
    if expected.len() != actual.len() {
        return f32::INFINITY;
    }
    expected
        .iter()
        .zip(actual)
        .map(|(a, b)| {
            let error = (a - b).abs();
            if error.is_finite() {
                error
            } else {
                f32::INFINITY
            }
        })
        .fold(0.0, f32::max)
}

mod matrix;
pub use matrix::{Compute, GpuError, Inference, Training};
