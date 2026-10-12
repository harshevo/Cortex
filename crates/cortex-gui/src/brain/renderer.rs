//! Drawing the brain on the GPU.
//!
//! Three passes share one camera uniform and are drawn back to front: the
//! glowing backdrop, the fibres, then the particles. Light only ever adds,
//! so nothing needs sorting or a depth buffer.
//!
//! Each frame the CPU sends one small uniform. The scenery is sent once,
//! and the memories only when they change.

use std::sync::Arc;

use eframe::egui;
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, RenderState, wgpu};
use wgpu::util::DeviceExt;

use super::instances::{Fibre, Frame, Globals, Particle, Snapshot};
use super::scenery;

const GLOBALS_SHADER: &str = include_str!("shaders/globals.wgsl");
const BACKDROP_SHADER: &str = include_str!("shaders/backdrop.wgsl");
const PARTICLES_SHADER: &str = include_str!("shaders/particles.wgsl");
const FIBRES_SHADER: &str = include_str!("shaders/fibres.wgsl");

/// Both instance types are four `vec4<f32>` in a row.
const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];

/// Instance buffers start with room for this many and double when full.
const INITIAL_CAPACITY: usize = 256;

/// Creates the GPU resources. Call once, before the first [`paint`].
pub fn install(render_state: &RenderState) {
    let renderer = BrainRenderer::new(&render_state.device, render_state.target_format);

    render_state
        .renderer
        .write()
        .callback_resources
        .insert(renderer);
}

/// A shape that draws `frame` into `rect` when egui paints it.
pub fn paint(rect: egui::Rect, frame: Frame) -> egui::Shape {
    egui::Shape::Callback(egui_wgpu::Callback::new_paint_callback(
        rect,
        FrameCallback { frame },
    ))
}

struct FrameCallback {
    frame: Frame,
}

impl CallbackTrait for FrameCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(renderer) = resources.get_mut::<BrainRenderer>() {
            renderer.upload(device, queue, &self.frame);
        }

        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        if let Some(renderer) = resources.get::<BrainRenderer>() {
            renderer.draw(pass);
        }
    }
}

/// A vertex buffer of instances that grows when it runs out of room.
struct InstanceBuffer {
    label: &'static str,
    buffer: wgpu::Buffer,
    capacity: usize,
    count: u32,
    stride: usize,
}

impl InstanceBuffer {
    fn new<T: bytemuck::Pod>(device: &wgpu::Device, label: &'static str) -> Self {
        let stride = size_of::<T>();

        Self {
            label,
            buffer: allocate(device, label, INITIAL_CAPACITY * stride),
            capacity: INITIAL_CAPACITY,
            count: 0,
            stride,
        }
    }

    fn write<T: bytemuck::Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[T],
    ) {
        if instances.len() > self.capacity {
            self.capacity = instances.len().next_power_of_two();
            self.buffer = allocate(device, self.label, self.capacity * self.stride);
        }

        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(instances));

        self.count = instances.len() as u32;
    }
}

fn allocate(device: &wgpu::Device, label: &str, bytes: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Instances that are made once and never change.
struct StaticBuffer {
    buffer: wgpu::Buffer,
    count: u32,
}

impl StaticBuffer {
    fn new<T: bytemuck::Pod>(device: &wgpu::Device, label: &str, instances: &[T]) -> Self {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(instances),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Self {
            buffer,
            count: instances.len() as u32,
        }
    }
}

struct BrainRenderer {
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,

    backdrop: wgpu::RenderPipeline,
    fibre_pipeline: wgpu::RenderPipeline,
    particle_pipeline: wgpu::RenderPipeline,

    dots: StaticBuffer,
    tracts: StaticBuffer,

    nodes: InstanceBuffer,
    links: InstanceBuffer,

    /// The revision of the memories the GPU currently holds.
    uploaded: Option<u64>,
}

impl BrainRenderer {
    fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brain globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brain globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brain globals"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brain"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let builder = PipelineBuilder {
            device,
            layout: &layout,
            target,
        };

        Self {
            globals,
            bind_group,

            backdrop: builder.build("brain backdrop", BACKDROP_SHADER, None, Blend::Replace),
            fibre_pipeline: builder.build(
                "brain fibres",
                FIBRES_SHADER,
                Some(size_of::<Fibre>()),
                Blend::Add,
            ),
            particle_pipeline: builder.build(
                "brain particles",
                PARTICLES_SHADER,
                Some(size_of::<Particle>()),
                Blend::Add,
            ),

            dots: StaticBuffer::new(device, "brain dots", &scenery::dots()),
            tracts: StaticBuffer::new(device, "brain tracts", &scenery::tracts()),

            nodes: InstanceBuffer::new::<Particle>(device, "brain nodes"),
            links: InstanceBuffer::new::<Fibre>(device, "brain links"),

            uploaded: None,
        }
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &Frame) {
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&frame.globals));

        let snapshot: &Arc<Snapshot> = &frame.snapshot;

        if self.uploaded == Some(snapshot.revision) {
            return;
        }

        self.nodes.write(device, queue, &snapshot.nodes);
        self.links.write(device, queue, &snapshot.links);

        self.uploaded = Some(snapshot.revision);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'static>) {
        pass.set_bind_group(0, &self.bind_group, &[]);

        pass.set_pipeline(&self.backdrop);
        pass.draw(0..3, 0..1);

        pass.set_pipeline(&self.fibre_pipeline);

        pass.set_vertex_buffer(0, self.tracts.buffer.slice(..));
        pass.draw(0..6, 0..self.tracts.count);

        if self.links.count > 0 {
            pass.set_vertex_buffer(0, self.links.buffer.slice(..));
            pass.draw(0..6, 0..self.links.count);
        }

        pass.set_pipeline(&self.particle_pipeline);

        pass.set_vertex_buffer(0, self.dots.buffer.slice(..));
        pass.draw(0..6, 0..self.dots.count);

        if self.nodes.count > 0 {
            pass.set_vertex_buffer(0, self.nodes.buffer.slice(..));
            pass.draw(0..6, 0..self.nodes.count);
        }
    }
}

#[derive(Clone, Copy)]
enum Blend {
    /// The backdrop: overwrite whatever is there.
    Replace,

    /// Light: add to whatever is there.
    Add,
}

impl Blend {
    fn state(self) -> wgpu::BlendState {
        match self {
            Self::Replace => wgpu::BlendState::REPLACE,

            Self::Add => {
                let add = wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                };

                wgpu::BlendState {
                    color: add,
                    alpha: add,
                }
            }
        }
    }
}

struct PipelineBuilder<'a> {
    device: &'a wgpu::Device,
    layout: &'a wgpu::PipelineLayout,
    target: wgpu::TextureFormat,
}

impl PipelineBuilder<'_> {
    /// `instance_stride` is the size of one instance, or `None` for a
    /// shader that draws without a vertex buffer.
    fn build(
        &self,
        label: &str,
        shader: &str,
        instance_stride: Option<usize>,
        blend: Blend,
    ) -> wgpu::RenderPipeline {
        let source = format!("{GLOBALS_SHADER}\n{shader}");

        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });

        let instances = instance_stride.map(|stride| wgpu::VertexBufferLayout {
            array_stride: stride as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &INSTANCE_ATTRIBUTES,
        });

        let buffers = match instances {
            Some(layout) => vec![Some(layout)],
            None => Vec::new(),
        };

        self.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(self.layout),

                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vertex"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &buffers,
                },

                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fragment"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.target,
                        blend: Some(blend.state()),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),

                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shader mistakes should fail a test, not blank the window.
    #[test]
    fn every_shader_is_valid_wgsl() {
        for (name, shader) in [
            ("backdrop", BACKDROP_SHADER),
            ("particles", PARTICLES_SHADER),
            ("fibres", FIBRES_SHADER),
        ] {
            let source = format!("{GLOBALS_SHADER}\n{shader}");

            let module = wgpu::naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(&source)));

            let mut validator = wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::empty(),
            );

            validator
                .validate(&module)
                .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        }
    }

    #[test]
    fn instances_match_the_shader_layout() {
        assert_eq!(size_of::<Particle>(), 64);
        assert_eq!(size_of::<Fibre>(), 64);
        assert_eq!(size_of::<Globals>(), 160);
    }
}
