use bytemuck::{Pod, Zeroable};
use dp_brush::Dab;

use crate::{Canvas, CANVAS_FORMAT};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    center: [f32; 2],
    radius: f32,
    hardness: f32,
    color: [f32; 4],
}

/// Rasterizes brush dabs into a canvas with "source over" blending.
///
/// Phase 0 paints straight into the canvas. The separate stroke buffer that
/// caps a stroke's opacity is part of the phase 1 brush engine.
pub struct DabRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    canvas_size: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: usize,
    pending: Vec<Instance>,
}

impl DabRenderer {
    const INITIAL_CAPACITY: usize = 1024;

    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("shaders/dab.wgsl"));
        let canvas_size = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dab canvas size"),
            // A vec2<f32> uniform is padded to 16 bytes on some backends.
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                        0 => Float32x2, 1 => Float32, 2 => Float32, 3 => Float32x4
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
                    format: CANVAS_FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dabs"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: canvas_size.as_entire_binding() }],
        });
        Self {
            pipeline,
            bind_group,
            canvas_size,
            instances: Self::instance_buffer(device, Self::INITIAL_CAPACITY),
            capacity: Self::INITIAL_CAPACITY,
            pending: Vec::new(),
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

    /// Queues dabs to be drawn by the next [`DabRenderer::flush`].
    ///
    /// `color` is straight (non-premultiplied) sRGB-encoded RGB.
    pub fn queue(&mut self, dabs: &[Dab], color: [f32; 3]) {
        self.pending.extend(dabs.iter().map(|dab| {
            let [r, g, b] = color.map(|c| c * dab.flow);
            Instance {
                center: dab.center.to_array(),
                radius: dab.radius,
                hardness: dab.hardness,
                color: [r, g, b, dab.flow],
            }
        }));
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Draws all queued dabs into `canvas`, in the order they were queued.
    pub fn flush(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        canvas: &Canvas,
    ) {
        if self.pending.is_empty() {
            return;
        }
        if self.pending.len() > self.capacity {
            self.capacity = self.pending.len().next_power_of_two();
            self.instances = Self::instance_buffer(device, self.capacity);
        }
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.pending));
        let size = [canvas.width() as f32, canvas.height() as f32, 0.0, 0.0];
        queue.write_buffer(&self.canvas_size, 0, bytemuck::cast_slice(&size));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("dabs"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: canvas.view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..6, 0..self.pending.len() as u32);
        drop(pass);
        self.pending.clear();
    }
}
