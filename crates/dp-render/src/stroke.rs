use bytemuck::{Pod, Zeroable};
use dp_brush::Dab;
use dp_core::Rect;

use crate::util::{color_pass, create_texture, extent, fullscreen_pipeline, texture_origin, uniform_buffer};
use crate::CANVAS_FORMAT;

/// Accumulated coverage needs more than 8 bits: low-flow dabs would
/// otherwise band and never reach full opacity.
const COVERAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

/// How a stroke's coverage is applied to its layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeStyle {
    /// Straight sRGB-encoded colour.
    pub color: [f32; 3],
    /// Ceiling for the opacity of the whole stroke.
    pub opacity: f32,
    pub erase: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    center: [f32; 2],
    radius: f32,
    hardness: f32,
    flow: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct StyleUniform {
    color: [f32; 3],
    opacity: f32,
    erase: u32,
    _padding: [u32; 3],
}

/// The stroke in progress.
///
/// Dabs accumulate in a separate coverage buffer and the layer itself stays
/// untouched until the stroke is committed. That keeps a stroke from building
/// up past its opacity where it overlaps itself, and lets undo capture the
/// layer exactly as it was.
pub(crate) struct StrokeLayer {
    size: (u32, u32),
    coverage: wgpu::TextureView,
    /// The layer with the stroke applied.
    preview_texture: wgpu::Texture,
    preview: wgpu::TextureView,
    dab_pipeline: wgpu::RenderPipeline,
    dab_bind_group: wgpu::BindGroup,
    merge_pipeline: wgpu::RenderPipeline,
    style: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: usize,
    pending: Vec<Instance>,
    needs_clear: bool,
}

impl StrokeLayer {
    const INITIAL_CAPACITY: usize = 1024;

    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32)) -> Self {
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let (_, coverage) = create_texture(device, "stroke coverage", size, COVERAGE_FORMAT, attachment);
        let (preview_texture, preview) = create_texture(
            device,
            "stroke preview",
            size,
            CANVAS_FORMAT,
            attachment | wgpu::TextureUsages::COPY_SRC,
        );

        let shader = device.create_shader_module(wgpu::include_wgsl!("shaders/dab.wgsl"));
        // A vec2<f32> uniform is padded to 16 bytes on some backends.
        let canvas_size = uniform_buffer(device, "dab canvas size", 16);
        queue.write_buffer(&canvas_size, 0, bytemuck::cast_slice(&[size.0 as f32, size.1 as f32, 0.0, 0.0]));
        // c' = s + c * (1 - s): each dab covers part of what is still uncovered.
        let accumulate = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrc,
            operation: wgpu::BlendOperation::Add,
        };
        let dab_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dabs"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2, 1 => Float32, 2 => Float32, 3 => Float32
                    ],
                })],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COVERAGE_FORMAT,
                    blend: Some(wgpu::BlendState { color: accumulate, alpha: accumulate }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let dab_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dabs"),
            layout: &dab_pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: canvas_size.as_entire_binding() }],
        });

        Self {
            size,
            coverage,
            preview_texture,
            preview,
            dab_pipeline,
            dab_bind_group,
            merge_pipeline: fullscreen_pipeline(
                device,
                "stroke merge",
                include_str!("shaders/stroke_merge.wgsl"),
                CANVAS_FORMAT,
            ),
            style: uniform_buffer(device, "stroke style", size_of::<StyleUniform>() as u64),
            instances: Self::instance_buffer(device, Self::INITIAL_CAPACITY),
            capacity: Self::INITIAL_CAPACITY,
            pending: Vec::new(),
            needs_clear: true,
        }
    }

    fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dab instances"),
            size: (capacity * size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Starts a new stroke, discarding the previous one's coverage.
    pub fn begin(&mut self) {
        self.pending.clear();
        self.needs_clear = true;
    }

    pub fn queue(&mut self, dabs: &[Dab]) {
        self.pending.extend(dabs.iter().map(|dab| Instance {
            center: dab.center.to_array(),
            radius: dab.radius,
            hardness: dab.hardness,
            flow: dab.flow,
        }));
    }

    /// The layer with the stroke applied, valid after [`StrokeLayer::render`].
    pub fn preview(&self) -> &wgpu::TextureView {
        &self.preview
    }

    /// Draws queued dabs into the coverage buffer, then rebuilds the preview
    /// of `layer` with the stroke applied.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        layer: &wgpu::TextureView,
        style: StrokeStyle,
    ) {
        if self.pending.len() > self.capacity {
            self.capacity = self.pending.len().next_power_of_two();
            self.instances = Self::instance_buffer(device, self.capacity);
        }
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.pending));
        let load = if std::mem::take(&mut self.needs_clear) {
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
        } else {
            wgpu::LoadOp::Load
        };
        {
            let mut pass = color_pass(encoder, "dabs", &self.coverage, load);
            if !self.pending.is_empty() {
                pass.set_pipeline(&self.dab_pipeline);
                pass.set_bind_group(0, &self.dab_bind_group, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                pass.draw(0..6, 0..self.pending.len() as u32);
            }
        }
        self.pending.clear();

        let uniform = StyleUniform {
            color: style.color,
            opacity: style.opacity.clamp(0.0, 1.0),
            erase: style.erase.into(),
            _padding: [0; 3],
        };
        queue.write_buffer(&self.style, 0, bytemuck::bytes_of(&uniform));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stroke merge"),
            layout: &self.merge_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(layer) },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.coverage),
                },
                wgpu::BindGroupEntry { binding: 2, resource: self.style.as_entire_binding() },
            ],
        });
        let mut pass = color_pass(encoder, "stroke merge", &self.preview, wgpu::LoadOp::Load);
        pass.set_pipeline(&self.merge_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Copies `rect` of the preview into the layer, making the stroke
    /// permanent. Call after [`StrokeLayer::render`] on the same encoder.
    pub fn commit(&self, encoder: &mut wgpu::CommandEncoder, layer: &wgpu::Texture, rect: Rect) {
        debug_assert!(rect.x + rect.width <= self.size.0 && rect.y + rect.height <= self.size.1);
        if rect.is_empty() {
            return;
        }
        encoder.copy_texture_to_texture(
            texture_origin(&self.preview_texture, rect),
            texture_origin(layer, rect),
            extent(rect.width, rect.height),
        );
    }
}
