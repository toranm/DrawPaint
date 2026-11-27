//! Small helpers shared by the render passes.

use dp_core::Rect;

pub(crate) fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d { width, height, depth_or_array_layers: 1 }
}

pub(crate) fn create_texture(
    device: &wgpu::Device,
    label: &str,
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: extent(width, height),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

pub(crate) fn uniform_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub(crate) fn texture_origin(texture: &wgpu::Texture, rect: Rect) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo {
        texture,
        mip_level: 0,
        origin: wgpu::Origin3d { x: rect.x, y: rect.y, z: 0 },
        aspect: wgpu::TextureAspect::All,
    }
}

/// A render pass that overwrites or loads a single colour target.
pub(crate) fn color_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    })
}

/// A pipeline that runs `fragment` over the whole target with no blending.
pub(crate) fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    fragment_source: &str,
    target: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let source = format!("{}\n{fragment_source}", include_str!("shaders/fullscreen.wgsl"));
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_fullscreen"),
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
                format: target,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Copies a rectangle of an RGBA8 texture back to the CPU as tightly packed
/// rows. Blocks until the GPU has finished, so it is not for the stroke path.
pub(crate) fn read_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    rect: Rect,
) -> Vec<u8> {
    if rect.is_empty() {
        return Vec::new();
    }
    const BYTES_PER_PIXEL: u32 = 4;
    let unpadded_row = rect.width * BYTES_PER_PIXEL;
    let padded_row = unpadded_row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded_row) * u64::from(rect.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture_origin(texture, rect),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: None,
            },
        },
        extent(rect.width, rect.height),
    );
    queue.submit([encoder.finish()]);

    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.expect("map readback buffer"));
    device.poll(wgpu::PollType::wait_indefinitely()).expect("wait for readback");

    let mapped = slice.get_mapped_range().expect("readback buffer is mapped");
    let mut pixels = Vec::with_capacity(rect.byte_len());
    for row in mapped.chunks_exact(padded_row as usize) {
        pixels.extend_from_slice(&row[..unpadded_row as usize]);
    }
    pixels
}
