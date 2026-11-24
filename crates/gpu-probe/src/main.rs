use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use bytemuck::cast_slice;
use wgpu::util::DeviceExt;

const M: usize = 128;
const K: usize = 128;
const N: usize = 128;
const GPU_SECONDS: f64 = 5.0;
const POWER_SECONDS: f64 = 5.0;
const POWER_INTERVAL: Duration = Duration::from_millis(200);
const DISPATCHES_PER_BATCH: u32 = 32;
const EPSILON: f32 = 1.0e-3;

#[derive(Debug, Clone)]
struct PowerSample {
    elapsed_seconds: f64,
    raw: String,
}

fn main() -> Result<(), String> {
    println!("gpu-probe version=0.1.0");
    println!("workload=matmul M={M} K={K} N={N} f32");
    println!(
        "arithmetic_intensity={:.3} FLOP/byte (2*M*K*N / (M*K*4 + K*N*4 + M*N*4))",
        arithmetic_intensity()
    );

    let adapters = enumerate_adapters()?;
    if adapters.is_empty() {
        return Err(
            "wgpu could not enumerate a Vulkan adapter: adapter list was empty".to_string(),
        );
    }

    for (index, adapter) in adapters.iter().enumerate() {
        let info = adapter.get_info();
        println!(
            "adapter[{index}] name={:?} backend={:?} driver={:?} driver_info={:?} device_type={:?}",
            info.name, info.backend, info.driver, info.driver_info, info.device_type
        );
    }

    let adapter = adapters
        .iter()
        .find(|adapter| adapter.get_info().device_type == wgpu::DeviceType::DiscreteGpu)
        .unwrap_or(&adapters[0]);
    let selected = adapter.get_info();
    println!(
        "selected_adapter name={:?} backend={:?} driver={:?} driver_info={:?}",
        selected.name, selected.backend, selected.driver, selected.driver_info
    );

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gpu-probe device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|error| format!("wgpu request_device failed: {error}"))?;

    let gpu = GpuMatmul::new(&device, &queue);
    let cpu_input = deterministic_input(M * K);
    let cpu_right = deterministic_input(K * N);
    let cpu_expected = cpu_matmul(&cpu_input, &cpu_right, M, K, N);
    let gpu_result = gpu.run_batch(&device, &queue, &cpu_input, &cpu_right, 1)?;
    let max_error = max_abs_error(&cpu_expected, &gpu_result);
    println!(
        "verification max_abs_error={max_error:.8e} epsilon={EPSILON:.8e} passed={}",
        max_error <= EPSILON
    );
    if max_error > EPSILON {
        return Err(format!(
            "GPU result exceeded epsilon: max_abs_error={max_error:.8e} epsilon={EPSILON:.8e}"
        ));
    }

    println!(
        "power_phase=idle duration_seconds={POWER_SECONDS:.1} interval_seconds={:.3}",
        POWER_INTERVAL.as_secs_f64()
    );
    let idle_samples = sample_power_for(POWER_SECONDS);
    print_power_samples("idle", &idle_samples);

    println!(
        "benchmark_phase=gpu_active duration_seconds={GPU_SECONDS:.1} dispatches_per_batch={DISPATCHES_PER_BATCH}"
    );
    let gpu_result = run_gpu_benchmark(&device, &queue, &gpu, &cpu_input, &cpu_right)?;
    print_power_samples("active", &gpu_result.power_samples);
    println!(
        "gpu_throughput mean_gflops={:.3} stdev_gflops={:.3} batches={} dispatches={} elapsed_seconds={:.6}",
        gpu_result.mean_gflops,
        gpu_result.stdev_gflops,
        gpu_result.batches,
        gpu_result.dispatches,
        gpu_result.elapsed_seconds
    );
    println!(
        "gpu_final_verification max_abs_error={:.8e} epsilon={EPSILON:.8e} passed={}",
        gpu_result.max_error,
        gpu_result.max_error <= EPSILON
    );

    println!("benchmark_phase=cpu duration_seconds={GPU_SECONDS:.1}");
    let cpu_result = run_cpu_benchmark(&cpu_input, &cpu_right, &cpu_expected);
    println!(
        "cpu_throughput mean_gflops={:.3} stdev_gflops={:.3} iterations={} elapsed_seconds={:.6}",
        cpu_result.mean_gflops,
        cpu_result.stdev_gflops,
        cpu_result.iterations,
        cpu_result.elapsed_seconds
    );

    Ok(())
}

fn enumerate_adapters() -> Result<Vec<wgpu::Adapter>, String> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });
    let adapters = instance.enumerate_adapters(wgpu::Backends::VULKAN);
    if adapters.is_empty() {
        println!("wgpu_enumeration_error=Vulkan adapter list was empty");
    } else {
        println!(
            "wgpu_enumeration=ok backend=Vulkan adapter_count={}",
            adapters.len()
        );
    }
    Ok(adapters)
}

struct GpuMatmul {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl GpuMatmul {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpu-probe matmul shader"),
            source: wgpu::ShaderSource::Wgsl(MATMUL_SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpu-probe matmul bind group layout"),
            entries: &[
                storage_entry(
                    0,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                storage_entry(
                    1,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                storage_entry(
                    2,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: false },
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu-probe matmul pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gpu-probe matmul pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            pipeline,
            bind_group_layout,
        }
    }

    fn run_batch(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        left: &[f32],
        right: &[f32],
        dispatches: u32,
    ) -> Result<Vec<f32>, String> {
        let left_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gpu-probe left"),
            contents: cast_slice(left),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let right_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gpu-probe right"),
            contents: cast_slice(right),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let output_size = (M * N * std::mem::size_of::<f32>()) as u64;
        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-probe output"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-probe readback"),
            size: output_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu-probe matmul bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: left_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: right_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output_buffer.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gpu-probe matmul encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("gpu-probe matmul pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            for _ in 0..dispatches {
                pass.dispatch_workgroups((N as u32).div_ceil(8), (M as u32).div_ceil(8), 1);
            }
        }
        encoder.copy_buffer_to_buffer(&output_buffer, 0, &readback, 0, output_size);
        queue.submit([encoder.finish()]);

        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| format!("wgpu device poll failed: {error}"))?;
        receiver
            .recv()
            .map_err(|error| format!("wgpu map callback failed: {error}"))?
            .map_err(|error| format!("wgpu readback map failed: {error}"))?;
        let mapped = slice.get_mapped_range();
        let values = cast_slice::<u8, f32>(&mapped).to_vec();
        drop(mapped);
        readback.unmap();
        Ok(values)
    }
}

struct GpuBenchmark {
    mean_gflops: f64,
    stdev_gflops: f64,
    batches: u64,
    dispatches: u64,
    elapsed_seconds: f64,
    max_error: f32,
    power_samples: Vec<PowerSample>,
}

fn run_gpu_benchmark(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    gpu: &GpuMatmul,
    left: &[f32],
    right: &[f32],
) -> Result<GpuBenchmark, String> {
    let start = Instant::now();
    let mut next_sample = start;
    let mut samples = Vec::new();
    let mut rates = Vec::new();
    let mut batches = 0_u64;
    let mut dispatches = 0_u64;
    let mut max_error = 0.0_f32;
    let expected = cpu_matmul(left, right, M, K, N);

    while start.elapsed().as_secs_f64() < GPU_SECONDS {
        let batch_start = Instant::now();
        let result = gpu.run_batch(device, queue, left, right, DISPATCHES_PER_BATCH)?;
        let batch_elapsed = batch_start.elapsed().as_secs_f64();
        let batch_flops = (2 * M * K * N * DISPATCHES_PER_BATCH as usize) as f64;
        rates.push(batch_flops / batch_elapsed / 1.0e9);
        max_error = max_error.max(max_abs_error(&expected, &result));
        batches += 1;
        dispatches += DISPATCHES_PER_BATCH as u64;

        if Instant::now() >= next_sample {
            samples.push(sample_power(start.elapsed().as_secs_f64()));
            next_sample += POWER_INTERVAL;
        }
    }
    let elapsed_seconds = start.elapsed().as_secs_f64();
    let (mean_gflops, stdev_gflops) = mean_and_stdev(&rates);
    Ok(GpuBenchmark {
        mean_gflops,
        stdev_gflops,
        batches,
        dispatches,
        elapsed_seconds,
        max_error,
        power_samples: samples,
    })
}

struct CpuBenchmark {
    mean_gflops: f64,
    stdev_gflops: f64,
    iterations: u64,
    elapsed_seconds: f64,
}

fn run_cpu_benchmark(left: &[f32], right: &[f32], expected: &[f32]) -> CpuBenchmark {
    let start = Instant::now();
    let mut rates = Vec::new();
    let mut iterations = 0_u64;
    let mut max_error = 0.0_f32;
    while start.elapsed().as_secs_f64() < GPU_SECONDS {
        let iteration_start = Instant::now();
        let result = cpu_matmul(left, right, M, K, N);
        let elapsed = iteration_start.elapsed().as_secs_f64();
        let flops = (2 * M * K * N) as f64;
        rates.push(flops / elapsed / 1.0e9);
        max_error = max_error.max(max_abs_error(expected, &result));
        iterations += 1;
        std::hint::black_box(result);
    }
    let elapsed_seconds = start.elapsed().as_secs_f64();
    let (mean_gflops, stdev_gflops) = mean_and_stdev(&rates);
    println!(
        "cpu_verification max_abs_error={max_error:.8e} epsilon={EPSILON:.8e} passed={}",
        max_error <= EPSILON
    );
    CpuBenchmark {
        mean_gflops,
        stdev_gflops,
        iterations,
        elapsed_seconds,
    }
}

fn sample_power(elapsed_seconds: f64) -> PowerSample {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=power.draw,utilization.gpu,clocks.sm",
            "--format=csv,noheader,nounits",
        ])
        .output();
    let raw = match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        Ok(output) => format!(
            "command_failed status={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => format!("command_error={error}"),
    };
    PowerSample {
        elapsed_seconds,
        raw,
    }
}

fn sample_power_for(seconds: f64) -> Vec<PowerSample> {
    let start = Instant::now();
    let mut next_sample = start;
    let mut samples = Vec::new();
    while start.elapsed().as_secs_f64() < seconds {
        if Instant::now() >= next_sample {
            samples.push(sample_power(start.elapsed().as_secs_f64()));
            next_sample += POWER_INTERVAL;
        } else {
            thread::sleep(Duration::from_millis(5));
        }
    }
    samples
}

fn print_power_samples(phase: &str, samples: &[PowerSample]) {
    println!("power_samples phase={phase} count={}", samples.len());
    for sample in samples {
        println!(
            "power_sample phase={phase} elapsed_seconds={:.3} values={:?}",
            sample.elapsed_seconds, sample.raw
        );
    }
}

fn deterministic_input(length: usize) -> Vec<f32> {
    (0..length)
        .map(|index| {
            let value = ((index as u64)
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223)
                % 10_000) as f32;
            value / 10_000.0 - 0.5
        })
        .collect()
}

fn cpu_matmul(left: &[f32], right: &[f32], rows: usize, inner: usize, columns: usize) -> Vec<f32> {
    let mut output = vec![0.0; rows * columns];
    for row in 0..rows {
        for column in 0..columns {
            let mut sum = 0.0;
            for depth in 0..inner {
                sum += left[row * inner + depth] * right[depth * columns + column];
            }
            output[row * columns + column] = sum;
        }
    }
    output
}

fn max_abs_error(expected: &[f32], actual: &[f32]) -> f32 {
    expected
        .iter()
        .zip(actual)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max)
}

fn arithmetic_intensity() -> f64 {
    (2 * M * K * N) as f64 / ((M * K + K * N + M * N) * std::mem::size_of::<f32>()) as f64
}

fn mean_and_stdev(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64;
    (mean, variance.sqrt())
}

fn storage_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

const MATMUL_SHADER: &str = r#"
const M: u32 = 128u;
const K: u32 = 128u;
const N: u32 = 128u;

@group(0) @binding(0) var<storage, read> left_matrix: array<f32>;
@group(0) @binding(1) var<storage, read> right_matrix: array<f32>;
@group(0) @binding(2) var<storage, read_write> output_matrix: array<f32>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let row = invocation.y;
    let column = invocation.x;
    if (row >= M || column >= N) {
        return;
    }
    var sum = 0.0;
    for (var depth = 0u; depth < K; depth = depth + 1u) {
        sum = sum + left_matrix[row * K + depth] * right_matrix[depth * N + column];
    }
    output_matrix[row * N + column] = sum;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_reference_kernel_is_deterministic() {
        let left = vec![1.0, 2.0, 3.0, 4.0];
        let right = vec![5.0, 6.0, 7.0, 8.0];
        assert_eq!(
            cpu_matmul(&left, &right, 2, 2, 2),
            vec![19.0, 22.0, 43.0, 50.0]
        );
    }

    #[test]
    fn epsilon_check_accepts_small_error_and_rejects_large_error() {
        let expected = [1.0, -2.0, 3.0];
        assert!(max_abs_error(&expected, &[1.0 + 0.5 * EPSILON, -2.0, 3.0]) <= EPSILON);
        assert!(max_abs_error(&expected, &[1.0 + 2.0 * EPSILON, -2.0, 3.0]) > EPSILON);
    }
}
