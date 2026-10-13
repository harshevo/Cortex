use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use super::{Brain, NeuralFrame};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Node {
    voltage: f32,
    refractory: u32,
    fired: u32,
    current: u32,
    slot: u32,
    gain: f32,
    drive: f32,
    feature: u32,
    count: u32,
    input: u32,
}

struct Device {
    device: wgpu::Device,
    queue: wgpu::Queue,
    name: String,
    layout: wgpu::BindGroupLayout,
    pipelines: Vec<wgpu::ComputePipeline>,
}

struct Wiring {
    offsets: wgpu::Buffer,
    targets: wgpu::Buffer,
    strengths: wgpu::Buffer,
}

pub(super) struct GpuBrain {
    device: Arc<Device>,
    _wiring: Arc<Wiring>,
    group: wgpu::BindGroup,
    nodes: wgpu::Buffer,
    features: wgpu::Buffer,
    counts: wgpu::Buffer,
    observations: wgpu::Buffer,
    readback: wgpu::Buffer,
    nodes_bytes: u64,
    counts_bytes: u64,
    trace_bytes: u64,
    samples: usize,
    steps: u32,
    neurons: usize,
}

fn buffer(
    device: &wgpu::Device,
    name: &str,
    data: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(name),
        contents: data,
        usage,
    })
}

impl Device {
    fn open() -> Result<Arc<Self>, String> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::METAL,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?;
            let name = adapter.get_info().name;
            let available = adapter.limits();
            let limits = wgpu::Limits {
                max_storage_buffer_binding_size: available.max_storage_buffer_binding_size,
                max_buffer_size: available.max_buffer_size,
                ..Default::default()
            };
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("Cortex simulation"),
                    required_limits: limits,
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?;
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("spiking kernels"),
                source: wgpu::ShaderSource::Wgsl(include_str!("spiking.wgsl").into()),
            });
            let entries: Vec<_> = (0..9)
                .map(|binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: if binding == 8 {
                            wgpu::BufferBindingType::Uniform
                        } else {
                            wgpu::BufferBindingType::Storage {
                                read_only: matches!(binding, 1 | 2 | 3 | 4 | 6),
                            }
                        },
                        has_dynamic_offset: binding == 8,
                        min_binding_size: None,
                    },
                    count: None,
                })
                .collect();
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("spiking layout"),
                entries: &entries,
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipelines = ["reset", "deliver", "update", "capture"]
                .iter()
                .map(|entry| {
                    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                        label: Some(entry),
                        layout: Some(&pipeline_layout),
                        module: &module,
                        entry_point: Some(entry),
                        compilation_options: Default::default(),
                        cache: None,
                    })
                })
                .collect();
            if let Some(error) = scope.pop().await {
                return Err(error.to_string());
            }
            Ok(Arc::new(Self {
                device,
                queue,
                name,
                layout,
                pipelines,
            }))
        })
    }
}

impl GpuBrain {
    pub(super) fn new(brain: &Brain) -> Result<Self, String> {
        static DEVICE: OnceLock<Result<Arc<Device>, String>> = OnceLock::new();
        static WIRING: OnceLock<Mutex<HashMap<usize, Weak<Wiring>>>> = OnceLock::new();
        let device = DEVICE.get_or_init(Device::open).clone()?;
        let mut cache = WIRING
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|e| e.to_string())?;
        let key = Arc::as_ptr(&brain.connectome) as usize;
        let wiring = match cache.get(&key).and_then(Weak::upgrade) {
            Some(wiring) => wiring,
            None => {
                let (offsets, targets, strengths) = brain.connectome.arrays();
                if std::mem::size_of_val(targets) as u64
                    > u64::from(device.device.limits().max_storage_buffer_binding_size)
                {
                    return Err("Connectome exceeds GPU buffer limit".into());
                }
                let packed: Vec<u32> = strengths
                    .chunks(2)
                    .map(|pair| {
                        u32::from(pair[0] as u16)
                            | (u32::from(pair.get(1).copied().unwrap_or(0) as u16) << 16)
                    })
                    .collect();
                let wiring = Arc::new(Wiring {
                    offsets: buffer(
                        &device.device,
                        "offsets",
                        bytemuck::cast_slice(offsets),
                        wgpu::BufferUsages::STORAGE,
                    ),
                    targets: buffer(
                        &device.device,
                        "targets",
                        bytemuck::cast_slice(targets),
                        wgpu::BufferUsages::STORAGE,
                    ),
                    strengths: buffer(
                        &device.device,
                        "strengths",
                        bytemuck::cast_slice(&packed),
                        wgpu::BufferUsages::STORAGE,
                    ),
                });
                cache.retain(|_, wiring| wiring.strong_count() > 0);
                cache.insert(key, Arc::downgrade(&wiring));
                wiring
            }
        };
        drop(cache);
        let mut state: Vec<Node> = (0..brain.config.neurons)
            .map(|id| Node {
                slot: brain.readout_slot[id],
                gain: brain.gain[id],
                ..Node::zeroed()
            })
            .collect();
        for (index, id) in brain.input_neurons.iter().enumerate() {
            state[*id as usize].input = 1;
            state[*id as usize].feature =
                (index * super::FEATURES / brain.input_neurons.len()) as u32;
            state[*id as usize].drive = brain.sensitivity[index] * super::INPUT_DRIVE;
        }
        let usage = wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST;
        let nodes = buffer(
            &device.device,
            "neurons",
            bytemuck::cast_slice(&state),
            usage,
        );
        let features = buffer(
            &device.device,
            "features",
            bytemuck::cast_slice(&[0.0_f32; super::FEATURES]),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let counts_bytes = u64::from(brain.config.steps) * 4;
        let counts = buffer(
            &device.device,
            "spike counts",
            &vec![0; counts_bytes as usize],
            usage,
        );
        let samples = buffer(
            &device.device,
            "sample IDs",
            bytemuck::cast_slice(&brain.telemetry.neuron_ids),
            wgpu::BufferUsages::STORAGE,
        );
        let trace_bytes =
            brain.telemetry.neuron_ids.len() as u64 * u64::from(brain.config.steps) * 8;
        let observations = buffer(
            &device.device,
            "observations",
            &vec![0; trace_bytes as usize],
            usage,
        );
        let mut config = vec![0_u32; brain.config.steps as usize * 64];
        for tick in 0..brain.config.steps as usize {
            config[tick * 64..tick * 64 + 8].copy_from_slice(&[
                brain.config.neurons as u32,
                tick as u32,
                brain.telemetry.neuron_ids.len() as u32,
                brain.config.steps,
                brain.config.central_gain.to_bits(),
                brain.config.global_inhibition.to_bits(),
                0,
                0,
            ]);
        }
        let config = buffer(
            &device.device,
            "simulation ticks",
            bytemuck::cast_slice(&config),
            wgpu::BufferUsages::UNIFORM,
        );
        let buffers = [
            &nodes,
            &wiring.offsets,
            &wiring.targets,
            &wiring.strengths,
            &features,
            &counts,
            &samples,
            &observations,
            &config,
        ];
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(binding, buffer)| wgpu::BindGroupEntry {
                binding: binding as u32,
                resource: if binding == 8 {
                    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(32),
                    })
                } else {
                    buffer.as_entire_binding()
                },
            })
            .collect();
        let group = device.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spiking bindings"),
            layout: &device.layout,
            entries: &entries,
        });
        let nodes_bytes = std::mem::size_of_val(state.as_slice()) as u64;
        let readback = device.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brain readback"),
            size: nodes_bytes + counts_bytes + trace_bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            device,
            _wiring: wiring,
            group,
            nodes,
            features,
            counts,
            observations,
            readback,
            nodes_bytes,
            counts_bytes,
            trace_bytes,
            samples: brain.telemetry.neuron_ids.len(),
            steps: brain.config.steps,
            neurons: brain.config.neurons,
        })
    }

    pub(super) fn name(&self) -> &str {
        &self.device.name
    }

    pub(super) fn respond(
        &self,
        features: &[f32; super::FEATURES],
        brain: &mut Brain,
    ) -> Result<Vec<f32>, String> {
        self.device
            .queue
            .write_buffer(&self.features, 0, bytemuck::cast_slice(features));
        let mut encoder = self
            .device
            .device
            .create_command_encoder(&Default::default());
        encoder.clear_buffer(&self.counts, 0, None);
        for tick in 0..self.steps {
            for pipeline in if tick == 0 {
                &[0, 1, 2, 3][..]
            } else {
                &[1, 2, 3][..]
            } {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&self.device.pipelines[*pipeline]);
                pass.set_bind_group(0, &self.group, &[tick * 256]);
                pass.dispatch_workgroups(
                    if *pipeline == 3 {
                        (self.samples as u32).div_ceil(128)
                    } else {
                        (self.neurons as u32).div_ceil(128)
                    },
                    1,
                    1,
                );
            }
        }
        encoder.copy_buffer_to_buffer(&self.nodes, 0, &self.readback, 0, self.nodes_bytes);
        encoder.copy_buffer_to_buffer(
            &self.counts,
            0,
            &self.readback,
            self.nodes_bytes,
            self.counts_bytes,
        );
        encoder.copy_buffer_to_buffer(
            &self.observations,
            0,
            &self.readback,
            self.nodes_bytes + self.counts_bytes,
            self.trace_bytes,
        );
        let submission = self.device.queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(5)),
            })
            .map_err(|e| e.to_string())?;
        receiver
            .recv_timeout(Duration::from_secs(1))
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let data = self
            .readback
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let nodes: &[Node] = bytemuck::cast_slice(&data[..self.nodes_bytes as usize]);
        let mut code = vec![0.0; brain.readout];
        for (id, node) in nodes.iter().enumerate() {
            brain.voltage[id] = node.voltage;
            brain.refractory[id] = node.refractory as u8;
            if node.slot != super::NOT_READ {
                code[node.slot as usize] = node.count as f32;
            }
        }
        let start = self.nodes_bytes as usize;
        let counts: &[u32] = bytemuck::cast_slice(&data[start..start + self.counts_bytes as usize]);
        brain.telemetry.total_spikes = counts.iter().map(|count| *count as usize).sum();
        let observations: &[u32] =
            bytemuck::cast_slice(&data[start + self.counts_bytes as usize..]);
        brain.telemetry.frames = observations
            .chunks(self.samples * 2)
            .map(|frame| NeuralFrame {
                voltage: frame
                    .chunks(2)
                    .map(|node| f32::from_bits(node[0]))
                    .collect(),
                spikes: frame.chunks(2).map(|node| node[1] != 0).collect(),
            })
            .collect();
        drop(data);
        self.readback.unmap();
        Ok(code)
    }
}
