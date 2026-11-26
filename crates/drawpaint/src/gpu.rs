//! Window surface and device setup.

use std::sync::Arc;

use anyhow::Context as _;
use winit::window::Window;

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

impl Gpu {
    pub fn new(window: Arc<Window>, display: winit::event_loop::OwnedDisplayHandle) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(display),
        ));
        let size = window.inner_size();
        let surface = instance.create_surface(window).context("create window surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("no compatible graphics adapter")?;
        tracing::info!(adapter = ?adapter.get_info(), "using graphics adapter");

        let (device, queue) = pollster::block_on(
            adapter
                .request_device(&wgpu::DeviceDescriptor { label: Some("drawpaint"), ..Default::default() }),
        )
        .context("create graphics device")?;

        let capabilities = surface.get_capabilities(&adapter);
        // Canvas values are already sRGB-encoded, so the surface must not
        // encode them a second time.
        let format = dp_render::preferred_surface_format(&capabilities.formats)
            .context("the surface offers no non-sRGB 8-bit format")?;
        let config = wgpu::SurfaceConfiguration {
            format,
            // One queued frame at most keeps pen-to-pixel latency low.
            desired_maximum_frame_latency: 1,
            ..surface
                .get_default_config(&adapter, size.width.max(1), size.height.max(1))
                .context("surface is not supported by the adapter")?
        };
        surface.configure(&device, &config);
        Ok(Self { device, queue, surface, config })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return; // Minimized.
        }
        self.config.width = width;
        self.config.height = height;
        self.reconfigure();
    }

    pub fn reconfigure(&self) {
        self.surface.configure(&self.device, &self.config);
    }

    pub fn size(&self) -> glam::Vec2 {
        glam::vec2(self.config.width as f32, self.config.height as f32)
    }
}
