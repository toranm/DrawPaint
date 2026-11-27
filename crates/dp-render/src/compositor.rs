use bytemuck::{Pod, Zeroable};
use dp_core::BlendMode;

use crate::util::{color_pass, create_texture, fullscreen_pipeline, uniform_buffer};
use crate::CANVAS_FORMAT;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    opacity: f32,
    mode: u32,
    _padding: [u32; 2],
}

/// One layer to composite, bottom first.
pub(crate) struct CompositeLayer<'a> {
    pub view: &'a wgpu::TextureView,
    pub opacity: f32,
    pub blend: BlendMode,
}

/// Flattens the layer stack into one texture.
///
/// Blend modes need to read what is below, which a render target cannot do
/// with itself, so layers are accumulated back and forth between two
/// textures.
pub(crate) struct LayerCompositor {
    targets: [(wgpu::Texture, wgpu::TextureView); 2],
    pipeline: wgpu::RenderPipeline,
    /// One uniform buffer per layer position, grown on demand.
    params: Vec<wgpu::Buffer>,
    /// Which target holds the latest result.
    result: usize,
}

impl LayerCompositor {
    pub fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        let target = |label| create_texture(device, label, size, CANVAS_FORMAT, usage);
        Self {
            targets: [target("composite a"), target("composite b")],
            pipeline: fullscreen_pipeline(
                device,
                "layer blend",
                include_str!("shaders/layer_blend.wgsl"),
                CANVAS_FORMAT,
            ),
            params: Vec::new(),
            result: 0,
        }
    }

    pub fn views(&self) -> [&wgpu::TextureView; 2] {
        [&self.targets[0].1, &self.targets[1].1]
    }

    pub fn result_index(&self) -> usize {
        self.result
    }

    pub fn result_texture(&self) -> &wgpu::Texture {
        &self.targets[self.result].0
    }

    /// Composites `layers` over an opaque `background`.
    pub fn composite(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        background: [f32; 3],
        layers: &[CompositeLayer<'_>],
    ) {
        let [r, g, b] = background.map(f64::from);
        let mut current = 0;
        drop(color_pass(
            encoder,
            "composite background",
            &self.targets[current].1,
            wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
        ));

        while self.params.len() < layers.len() {
            self.params.push(uniform_buffer(device, "layer blend", size_of::<Params>() as u64));
        }
        for (layer, params) in layers.iter().zip(&self.params) {
            let uniform = Params {
                opacity: layer.opacity.clamp(0.0, 1.0),
                mode: match layer.blend {
                    BlendMode::Normal => 0,
                    BlendMode::Multiply => 1,
                    BlendMode::Screen => 2,
                    BlendMode::Overlay => 3,
                    BlendMode::Add => 4,
                },
                _padding: [0; 2],
            };
            queue.write_buffer(params, 0, bytemuck::bytes_of(&uniform));
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("layer blend"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&self.targets[current].1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(layer.view),
                    },
                    wgpu::BindGroupEntry { binding: 2, resource: params.as_entire_binding() },
                ],
            });
            let next = 1 - current;
            let mut pass = color_pass(encoder, "layer blend", &self.targets[next].1, wgpu::LoadOp::Load);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
            drop(pass);
            current = next;
        }
        self.result = current;
    }
}
