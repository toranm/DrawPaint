use std::collections::HashMap;

use dp_core::{LayerId, PixelStore, Rect};

use crate::util::{create_texture, extent, read_rgba8, texture_origin};
use crate::CANVAS_FORMAT;

pub(crate) struct LayerTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

/// Layer pixels held in GPU textures, one full-canvas texture per layer.
pub(crate) struct LayerPixels {
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: (u32, u32),
    layers: HashMap<LayerId, LayerTexture>,
}

impl LayerPixels {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, size: (u32, u32)) -> Self {
        Self { device, queue, size, layers: HashMap::new() }
    }

    pub fn get(&self, layer: LayerId) -> Option<&LayerTexture> {
        self.layers.get(&layer)
    }
}

impl PixelStore for LayerPixels {
    fn read_rect(&mut self, layer: LayerId, rect: Rect) -> Vec<u8> {
        match self.layers.get(&layer) {
            Some(layer) => read_rgba8(&self.device, &self.queue, &layer.texture, rect),
            None => vec![0; rect.byte_len()],
        }
    }

    fn write_rect(&mut self, layer: LayerId, rect: Rect, pixels: &[u8]) {
        assert_eq!(pixels.len(), rect.byte_len(), "pixel data does not match its rectangle");
        let Some(layer) = self.layers.get(&layer) else { return };
        if rect.is_empty() {
            return;
        }
        self.queue.write_texture(
            texture_origin(&layer.texture, rect),
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(rect.width * 4),
                rows_per_image: None,
            },
            extent(rect.width, rect.height),
        );
    }

    fn create_layer(&mut self, layer: LayerId) {
        // wgpu zero-initializes textures, so a new layer is transparent.
        let (texture, view) = create_texture(
            &self.device,
            "layer",
            self.size,
            CANVAS_FORMAT,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
        );
        self.layers.insert(layer, LayerTexture { texture, view });
    }

    fn delete_layer(&mut self, layer: LayerId) {
        if let Some(layer) = self.layers.remove(&layer) {
            layer.texture.destroy();
        }
    }
}
