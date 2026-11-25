use crate::model::{Features, INPUTS, OUTPUTS, SyntheticMlpCostModel, SyntheticOutput};
use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
use wgpu::{BufferUsages as Usage, util::DeviceExt};

const LANES: u32 = 64;
pub const INPUT_BYTES: u64 = bytes(INPUTS);
pub const READBACK_BYTES: u64 = bytes(OUTPUTS);

const fn bytes(count: usize) -> u64 {
    (count * size_of::<f32>()) as u64
}

pub struct Gpu {
    pub adapter: wgpu::AdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
    error: Arc<Mutex<Option<String>>>,
    layers: [Dispatch; 3],
    input: wgpu::Buffer,
    upload: wgpu::Buffer,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
}

struct Dispatch {
    pipeline: wgpu::ComputePipeline,
    bindings: wgpu::BindGroup,
    rows: u32,
}

impl Gpu {
    pub fn new(model: &SyntheticMlpCostModel) -> Result<Self, String> {
        let adapter = gpu_probe::vulkan_adapters()
            .into_iter()
            .find(|adapter| {
                let info = adapter.get_info();
                info.vendor == 0x10de
                    && matches!(
                        info.device_type,
                        wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::IntegratedGpu
                    )
            })
            .ok_or_else(|| "no physical NVIDIA Vulkan adapter".to_owned())?;
        let (device, queue) = gpu_probe::open_device(&adapter)?;
        let limits = device.limits();
        for layer in &model.layers {
            let size = bytes(layer.parameters.len());
            if size > u64::from(limits.max_storage_buffer_binding_size)
                || size > limits.max_buffer_size
                || layer.outputs > limits.max_compute_workgroups_per_dimension as usize
            {
                return Err(format!(
                    "layer {}x{} ({size} bytes) exceeds device limits",
                    layer.inputs, layer.outputs
                ));
            }
        }
        if LANES > limits.max_compute_workgroup_size_x
            || LANES > limits.max_compute_invocations_per_workgroup
            || bytes(LANES as usize) > u64::from(limits.max_compute_workgroup_storage_size)
        {
            return Err("workgroup exceeds device limits".to_owned());
        }
        let error = Arc::new(Mutex::new(None));
        let capture = Arc::clone(&error);
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            capture
                .lock()
                .unwrap()
                .get_or_insert_with(|| error.to_string());
        }));
        let capture = Arc::clone(&error);
        device.set_device_lost_callback(move |reason, message| {
            capture
                .lock()
                .unwrap()
                .get_or_insert_with(|| format!("{reason:?}: {message}"));
        });
        let buffer = |label, size, usage, mapped_at_creation| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation,
            })
        };
        let input = buffer(
            "device input",
            INPUT_BYTES,
            Usage::STORAGE | Usage::COPY_DST,
            false,
        );
        let hidden_a = buffer(
            "hidden A",
            bytes(model.layers[0].outputs),
            Usage::STORAGE,
            false,
        );
        let hidden_b = buffer(
            "hidden B",
            bytes(model.layers[1].outputs),
            Usage::STORAGE,
            false,
        );
        let output = buffer(
            "device output",
            READBACK_BYTES,
            Usage::STORAGE | Usage::COPY_SRC,
            false,
        );
        let upload = buffer(
            "host input",
            INPUT_BYTES,
            Usage::MAP_WRITE | Usage::COPY_SRC,
            true,
        );
        let readback = buffer(
            "host output",
            READBACK_BYTES,
            Usage::MAP_READ | Usage::COPY_DST,
            false,
        );
        let activation_pairs = [
            (&input, &hidden_a),
            (&hidden_a, &hidden_b),
            (&hidden_b, &output),
        ];
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("MLP dense bindings"),
            entries: &std::array::from_fn::<_, 3, _>(|binding| wgpu::BindGroupLayoutEntry {
                binding: binding as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: binding != 2,
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let layers = std::array::from_fn(|index| {
            let layer = &model.layers[index];
            let (input, output) = activation_pairs[index];
            let source = format!(
                "const LANES = {LANES}u; const INPUTS = {}u; const OUTPUTS = {}u; const RELU = {};\n{}",
                layer.inputs,
                layer.outputs,
                layer.relu,
                include_str!("dense.wgsl")
            );
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("MLP dense"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("MLP dense"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("dense"),
                compilation_options: Default::default(),
                cache: None,
            });
            let parameters = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("resident parameters"),
                contents: bytemuck::cast_slice(&layer.parameters),
                usage: Usage::STORAGE,
            });
            let buffers = [input, &parameters, output];
            let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &std::array::from_fn::<_, 3, _>(|binding| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: buffers[binding].as_entire_binding(),
                }),
            });
            Dispatch {
                pipeline,
                bindings,
                rows: layer.outputs as u32,
            }
        });
        let gpu = Self {
            adapter: adapter.get_info(),
            device,
            queue,
            error,
            layers,
            input,
            upload,
            output,
            readback,
        };
        gpu.check_error()?;
        Ok(gpu)
    }

    pub fn infer(&self, features: &Features) -> Result<SyntheticOutput, String> {
        self.upload
            .slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(bytemuck::cast_slice(features));
        self.upload.unmap();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_buffer_to_buffer(&self.upload, 0, &self.input, 0, INPUT_BYTES);
        for layer in &self.layers {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&layer.pipeline);
            pass.set_bind_group(0, &layer.bindings, &[]);
            pass.dispatch_workgroups(layer.rows, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.output, 0, &self.readback, 0, READBACK_BYTES);

        let (sender, map_results) = mpsc::channel();
        let upload_ready = sender.clone();
        encoder.map_buffer_on_submit(&self.upload, wgpu::MapMode::Write, .., move |result| {
            let _ = upload_ready.send(("upload", result));
        });
        encoder.map_buffer_on_submit(&self.readback, wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(("readback", result));
        });
        let index = self.queue.submit([encoder.finish()]);
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: Some(Duration::from_secs(10)),
            })
            .map_err(|e| format!("GPU submission wait: {e}"))?;
        self.check_error()?;
        for _ in 0..2 {
            let (buffer, result) = map_results
                .try_recv()
                .map_err(|e| format!("GPU mapping callback: {e}"))?;
            result.map_err(|e| format!("GPU {buffer} mapping: {e}"))?;
        }
        let data = self.readback.slice(..).get_mapped_range();
        let output: SyntheticOutput = bytemuck::cast_slice(&data).try_into().unwrap();
        drop(data);
        self.readback.unmap();
        Ok(output)
    }

    fn check_error(&self) -> Result<(), String> {
        match self.error.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cpu::Scalar,
        model::{EPSILON, features},
    };

    #[test]
    fn sizes_cover_parameters_and_both_transfer_directions() {
        assert_eq!(INPUT_BYTES, 256);
        assert_eq!(READBACK_BYTES, 16);
        for (width, expected) in [
            (71, 10_015),
            (283, 99_903),
            (966, 1_000_780),
            (1965, 3_998_779),
        ] {
            let model = SyntheticMlpCostModel::new(width, 42);
            assert_eq!(model.parameters(), expected);
            assert_eq!(model.layers[1].parameters.len(), width * (width + 1));
            assert_eq!(bytes(model.parameters()), expected as u64 * 4);
        }
    }

    #[test]
    fn gpu_matches_cpu_reference_for_consecutive_inputs() {
        let model = SyntheticMlpCostModel::new(71, 42);
        let gpu = match Gpu::new(&model) {
            Ok(gpu) => gpu,
            Err(error) if error == "no physical NVIDIA Vulkan adapter" => {
                eprintln!("SKIP predict-cost GPU test: {error}");
                return;
            }
            Err(error) => panic!("{error}"),
        };
        let mut scalar = Scalar::new(&model);
        for input in features(42) {
            let actual = gpu.infer(&input).unwrap();
            assert!(gpu_probe::max_abs_error(&scalar.infer(&input), &actual) <= EPSILON);
        }
    }
}
