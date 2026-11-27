//! GPU side of DrawPaint: layer pixels, brush rasterization, layer
//! compositing and the on-screen presentation of the canvas.
//!
//! Colours are stored as sRGB-encoded RGBA8 with premultiplied alpha and are
//! blended in that encoded space, which is what painters coming from other
//! raster apps expect. For that reason every texture here uses a non-sRGB
//! format: the GPU must not linearize the values.

mod canvas;
mod compositor;
mod pixels;
mod present;
mod stroke;
mod util;

pub use canvas::CanvasRenderer;
pub use stroke::StrokeStyle;

/// Format of layer and composite textures.
pub const CANVAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Picks the surface format that shows canvas values unchanged.
///
/// Returns `None` if the surface only offers sRGB formats.
pub fn preferred_surface_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    formats
        .iter()
        .copied()
        .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
}
