use crate::CANVAS_FORMAT;

/// The pixels of the document. Phase 0 has a single opaque layer.
pub struct Canvas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl Canvas {
    /// Largest canvas side that fits on this device.
    pub fn max_side(device: &wgpu::Device) -> u32 {
        device.limits().max_texture_dimension_2d
    }

    /// Creates a canvas with undefined contents; call [`Canvas::clear`] before use.
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CANVAS_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view, width, height }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Fills the whole canvas with a premultiplied colour.
    pub fn clear(&self, encoder: &mut wgpu::CommandEncoder, color: [f32; 4]) {
        let [r, g, b, a] = color.map(f64::from);
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("canvas clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }

    /// Copies the canvas back to the CPU as tightly packed RGBA8 rows.
    ///
    /// Blocks until the GPU has finished; not for use on the stroke path.
    pub fn read_pixels(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
        const BYTES_PER_PIXEL: u32 = 4;
        let unpadded_row = self.width * BYTES_PER_PIXEL;
        let padded_row = unpadded_row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("canvas readback"),
            size: u64::from(padded_row) * u64::from(self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |result| result.expect("map canvas readback buffer"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("wait for canvas readback");

        let mapped = slice.get_mapped_range().expect("canvas readback buffer is mapped");
        let mut pixels = Vec::with_capacity((unpadded_row * self.height) as usize);
        for row in mapped.chunks_exact(padded_row as usize) {
            pixels.extend_from_slice(&row[..unpadded_row as usize]);
        }
        pixels
    }
}
