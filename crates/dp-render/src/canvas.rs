use dp_brush::Dab;
use dp_core::{Document, LayerId, PixelStore, Rect};
use glam::{Affine2, Vec2};

use crate::compositor::{CompositeLayer, LayerCompositor};
use crate::pixels::LayerPixels;
use crate::present::CanvasPresenter;
use crate::stroke::{StrokeLayer, StrokeStyle};
use crate::util::read_rgba8;

/// Everything on the GPU that belongs to one document: its layers, the
/// stroke in progress, the flattened image and its on-screen presentation.
///
/// A renderer is tied to its document's canvas size; a new or opened
/// document gets a new renderer.
pub struct CanvasRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: (u32, u32),
    pixels: LayerPixels,
    stroke: StrokeLayer,
    compositor: LayerCompositor,
    presenter: CanvasPresenter,
}

impl CanvasRenderer {
    /// Largest canvas side the device can hold.
    pub fn max_side(device: &wgpu::Device) -> u32 {
        device.limits().max_texture_dimension_2d
    }

    /// Creates the GPU state for `document`, with all its layers transparent.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
        document: &Document,
    ) -> Self {
        let size = (document.width(), document.height());
        let mut pixels = LayerPixels::new(device.clone(), queue.clone(), size);
        for layer in document.layers() {
            pixels.create_layer(layer.id);
        }
        let compositor = LayerCompositor::new(device, size);
        let presenter = CanvasPresenter::new(device, target_format, size, &compositor.views());
        Self {
            device: device.clone(),
            queue: queue.clone(),
            size,
            pixels,
            stroke: StrokeLayer::new(device, queue, size),
            compositor,
            presenter,
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect::from_size(self.size.0, self.size.1)
    }

    /// The layer pixels, for undo/redo and file loading.
    pub fn pixels(&mut self) -> &mut dyn PixelStore {
        &mut self.pixels
    }

    /// Starts a new stroke with an empty coverage buffer.
    pub fn begin_stroke(&mut self) {
        self.stroke.begin();
    }

    /// Adds dabs to the stroke in progress.
    pub fn queue_dabs(&mut self, dabs: &[Dab]) {
        self.stroke.queue(dabs);
    }

    /// Applies the stroke in progress to `layer` permanently and returns the
    /// pixels `rect` held before, for undo. `rect` must cover every dab of
    /// the stroke.
    pub fn commit_stroke(&mut self, layer: LayerId, rect: Rect, style: StrokeStyle) -> Vec<u8> {
        let before = self.pixels.read_rect(layer, rect);
        if let Some(target) = self.pixels.get(layer) {
            let mut encoder = self.device.create_command_encoder(&Default::default());
            self.stroke.render(&self.device, &self.queue, &mut encoder, &target.view, style);
            self.stroke.commit(&mut encoder, &target.texture, rect);
            self.queue.submit([encoder.finish()]);
        }
        self.stroke.begin();
        before
    }

    /// Flattens the visible layers. `stroke` names the layer being painted
    /// on, if any, so the stroke in progress shows in place.
    pub fn composite(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        document: &Document,
        stroke: Option<(LayerId, StrokeStyle)>,
    ) {
        if let Some((id, style)) = stroke {
            if let Some(layer) = self.pixels.get(id) {
                self.stroke.render(&self.device, &self.queue, encoder, &layer.view, style);
            }
        }
        let layers: Vec<CompositeLayer<'_>> = document
            .layers()
            .iter()
            .filter(|layer| layer.props.visible && layer.props.opacity > 0.0)
            .filter_map(|layer| {
                let painting = stroke.is_some_and(|(id, _)| id == layer.id);
                let view = if painting { self.stroke.preview() } else { &self.pixels.get(layer.id)?.view };
                Some(CompositeLayer { view, opacity: layer.props.opacity, blend: layer.props.blend })
            })
            .collect();
        self.compositor.composite(&self.device, &self.queue, encoder, document.background, &layers);
    }

    /// Updates the view. `canvas_to_screen` maps canvas pixels to physical
    /// window pixels; `target_size` is the window size in physical pixels.
    pub fn set_view(&mut self, canvas_to_screen: Affine2, target_size: Vec2) {
        self.presenter.set_view(&self.queue, canvas_to_screen, target_size);
    }

    /// Draws the latest composite into the window.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.presenter.draw(pass, self.compositor.result_index());
    }

    /// Reads back part of the latest composite as opaque RGBA8.
    pub fn read_composite(&self, rect: Rect) -> Vec<u8> {
        read_rgba8(&self.device, &self.queue, self.compositor.result_texture(), rect)
    }

    /// Composites `document` and reads the whole flattened image back.
    pub fn flatten(&mut self, document: &Document) -> Vec<u8> {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.composite(&mut encoder, document, None);
        self.queue.submit([encoder.finish()]);
        self.read_composite(self.bounds())
    }
}
