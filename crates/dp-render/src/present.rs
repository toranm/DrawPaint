use bytemuck::{Pod, Zeroable};
use glam::{Affine2, Mat4, Vec2, Vec3};

use crate::Canvas;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ViewUniform {
    transform: [[f32; 4]; 4],
    canvas_size: [f32; 2],
    _padding: [f32; 2],
}

/// Draws a canvas into the window through a view transform.
pub struct CanvasPresenter {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    /// Smooth sampling for zoomed-out views.
    linear: wgpu::BindGroup,
    /// Crisp pixels for zoomed-in views.
    nearest: wgpu::BindGroup,
    use_nearest: bool,
}

impl CanvasPresenter {
    /// Zoom from which individual canvas pixels are shown unfiltered.
    const NEAREST_FROM_ZOOM: f32 = 2.0;

    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat, canvas: &Canvas) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("shaders/present.wgsl"));
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("canvas view"),
            size: size_of::<ViewUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("canvas present"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let bind_group = |filter: wgpu::FilterMode| {
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("canvas"),
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("canvas present"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(canvas.view()),
                    },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                ],
            })
        };
        Self {
            linear: bind_group(wgpu::FilterMode::Linear),
            nearest: bind_group(wgpu::FilterMode::Nearest),
            pipeline,
            uniform,
            use_nearest: false,
        }
    }

    /// Updates the view. `canvas_to_screen` maps canvas pixels to physical
    /// window pixels; `target_size` is the window size in physical pixels.
    pub fn set_view(
        &mut self,
        queue: &wgpu::Queue,
        canvas: &Canvas,
        canvas_to_screen: Affine2,
        target_size: Vec2,
    ) {
        let screen_to_clip = Mat4::from_translation(Vec3::new(-1.0, 1.0, 0.0))
            * Mat4::from_scale(Vec3::new(2.0 / target_size.x, -2.0 / target_size.y, 1.0));
        let m = canvas_to_screen.matrix2;
        let t = canvas_to_screen.translation;
        let canvas_to_screen = Mat4::from_cols_array_2d(&[
            [m.x_axis.x, m.x_axis.y, 0.0, 0.0],
            [m.y_axis.x, m.y_axis.y, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [t.x, t.y, 0.0, 1.0],
        ]);
        let uniform = ViewUniform {
            transform: (screen_to_clip * canvas_to_screen).to_cols_array_2d(),
            canvas_size: [canvas.width() as f32, canvas.height() as f32],
            _padding: [0.0; 2],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        let zoom = m.x_axis.length();
        self.use_nearest = zoom >= Self::NEAREST_FROM_ZOOM;
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, if self.use_nearest { &self.nearest } else { &self.linear }, &[]);
        pass.draw(0..6, 0..1);
    }
}
