//! The open document and everything that acts on it: painting, layers,
//! undo, files. Independent of the window and of egui's widgets.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use dp_brush::{Dab, StrokeBuilder};
use dp_core::{Document, Edit, History, LayerId, PixelStore, Rect, Snapshot, ViewTransform};
use dp_input::{DeviceKind, Phase, PointerSample};
use dp_render::{CanvasRenderer, StrokeStyle};
use dp_ui::{Action, Frame, PointerInfo, UiState};
use glam::{vec2, Vec2};

const DEFAULT_CANVAS_SIDE: u32 = 2048;
/// Memory the undo history may hold before dropping its oldest steps.
const HISTORY_BUDGET_BYTES: usize = 512 * 1024 * 1024;
const IMAGE_EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

/// An operation that would lose unsaved changes and so may need the user's
/// confirmation first.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Guarded {
    New { width: u32, height: u32 },
    Open,
    Close,
}

struct Stroke {
    builder: StrokeBuilder,
    layer: LayerId,
    style: StrokeStyle,
    /// Bounds of every dab so far, clipped to the canvas.
    rect: Rect,
}

pub struct Session {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target_format: wgpu::TextureFormat,

    document: Document,
    renderer: CanvasRenderer,
    history: History,
    path: Option<PathBuf>,
    modified: bool,

    pub view: ViewTransform,
    fit_pending: bool,
    stroke: Option<Stroke>,
    dab_scratch: Vec<Dab>,
    composite_dirty: bool,

    /// Operation waiting for the unsaved-changes answer.
    awaiting_confirmation: Option<Guarded>,
    error: Option<String>,
    exit_requested: bool,
}

impl Session {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) -> anyhow::Result<Self> {
        let side = DEFAULT_CANVAS_SIDE.min(CanvasRenderer::max_side(device));
        let document = Document::new(side, side).context("create the initial canvas")?;
        let renderer = CanvasRenderer::new(device, queue, target_format, &document);
        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            target_format,
            document,
            renderer,
            history: History::new(HISTORY_BUDGET_BYTES),
            path: None,
            modified: false,
            view: ViewTransform::default(),
            fit_pending: true,
            stroke: None,
            dab_scratch: Vec::new(),
            composite_dirty: true,
            awaiting_confirmation: None,
            error: None,
            exit_requested: false,
        })
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_requested
    }

    pub fn is_painting(&self) -> bool {
        self.stroke.is_some()
    }

    /// Whether a modal dialog is waiting for an answer, so the canvas and
    /// shortcuts must not react.
    pub fn is_blocked(&self) -> bool {
        self.awaiting_confirmation.is_some() || self.error.is_some()
    }

    pub fn window_title(&self) -> String {
        let name = self.file_name().unwrap_or("Sin título");
        let marker = if self.modified { " •" } else { "" };
        format!("{name}{marker} — DrawPaint")
    }

    fn file_name(&self) -> Option<&str> {
        self.path.as_deref().and_then(Path::file_name).and_then(|name| name.to_str())
    }

    pub fn frame(&self, pointer: PointerInfo, max_canvas_side: u32) -> Frame<'_> {
        Frame {
            document: &self.document,
            file_name: self.file_name(),
            modified: self.modified,
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
            zoom: self.view.zoom,
            rotation_degrees: self.view.rotation.to_degrees(),
            max_canvas_side,
            pointer,
            confirm_unsaved: self.awaiting_confirmation.is_some(),
            error: self.error.as_deref(),
        }
    }

    /// The window's close button was pressed.
    pub fn request_close(&mut self) {
        self.guarded(Guarded::Close);
    }

    // --- Painting -------------------------------------------------------

    /// Handles one pointer sample. `may_start` says whether a stroke may
    /// begin at this sample (it is on the canvas and nothing else claims it).
    pub fn pointer(&mut self, sample: &PointerSample, may_start: bool, pick_color: bool, ui: &mut UiState) {
        let pos = self.view.to_canvas(sample.pos);
        match sample.phase {
            Phase::Down if may_start && pick_color => self.pick_color(pos, ui),
            Phase::Down if may_start => self.begin_stroke(pos, sample, ui),
            Phase::Move => {
                if let Some(stroke) = &mut self.stroke {
                    stroke.builder.extend(pos, sample.pressure, &mut self.dab_scratch);
                    self.flush_dabs();
                }
            }
            // The lift-off sample carries no pressure; the stroke ends where
            // the last contact sample left it.
            Phase::Up => self.finish_stroke(),
            Phase::Down | Phase::Hover => {}
        }
    }

    fn begin_stroke(&mut self, pos: Vec2, sample: &PointerSample, ui: &UiState) {
        self.finish_stroke();
        let layer = self.document.active_layer();
        if !layer.props.visible {
            self.error = Some("La capa activa está oculta. Hazla visible para pintar en ella.".to_owned());
            return;
        }
        let style = StrokeStyle {
            color: ui.color,
            opacity: ui.brush.opacity,
            erase: ui.brush.erase || sample.device == DeviceKind::Eraser,
        };
        self.renderer.begin_stroke();
        let builder = StrokeBuilder::begin(ui.brush, pos, sample.pressure, &mut self.dab_scratch);
        self.stroke = Some(Stroke { builder, layer: layer.id, style, rect: Rect::EMPTY });
        self.flush_dabs();
    }

    /// Sends freshly generated dabs of the stroke in progress to the renderer.
    fn flush_dabs(&mut self) {
        let Some(rect) = self.stroke.as_ref().map(|stroke| stroke.rect) else { return };
        let rect = self.queue_dabs(rect);
        if let Some(stroke) = &mut self.stroke {
            stroke.rect = rect;
        }
    }

    /// Queues the dabs in `dab_scratch` and returns `rect` grown to cover them.
    fn queue_dabs(&mut self, mut rect: Rect) -> Rect {
        let bounds = self.renderer.bounds();
        for dab in &self.dab_scratch {
            // One pixel of margin for the antialiased edge.
            rect = rect.union(Rect::around_circle(dab.center, dab.radius + 1.0, bounds));
        }
        if !self.dab_scratch.is_empty() {
            self.renderer.queue_dabs(&self.dab_scratch);
            self.dab_scratch.clear();
            self.composite_dirty = true;
        }
        rect
    }

    /// Ends the stroke in progress, if any, and makes it undoable.
    fn finish_stroke(&mut self) {
        let Some(Stroke { builder, layer, style, rect }) = self.stroke.take() else { return };
        builder.finish(&mut self.dab_scratch);
        let rect = self.queue_dabs(rect);
        if !rect.is_empty() {
            let before = self.renderer.commit_stroke(layer, rect, style);
            self.history.record(Edit::Pixels { layer, snapshot: Snapshot::new(rect, &before) });
            self.modified = true;
        }
        self.composite_dirty = true;
    }

    fn pick_color(&mut self, pos: Vec2, ui: &mut UiState) {
        let bounds = self.renderer.bounds();
        if pos.x < 0.0 || pos.y < 0.0 || pos.x >= bounds.width as f32 || pos.y >= bounds.height as f32 {
            return;
        }
        self.refresh_composite();
        let pixel = self.renderer.read_composite(Rect::new(pos.x as u32, pos.y as u32, 1, 1));
        if let [r, g, b, _] = pixel[..] {
            ui.color = [r, g, b].map(|c| f32::from(c) / 255.0);
        }
    }

    // --- Actions --------------------------------------------------------

    /// Performs something the user asked for through the UI or a shortcut.
    /// `canvas_rect` is the canvas area in physical pixels.
    pub fn apply(&mut self, action: Action, canvas_rect: egui::Rect) {
        // The few actions allowed while a dialog is open.
        match action {
            Action::DismissError => {
                self.error = None;
                return;
            }
            Action::UnsavedCancel => {
                self.awaiting_confirmation = None;
                return;
            }
            Action::UnsavedDiscard => {
                if let Some(guarded) = self.awaiting_confirmation.take() {
                    self.run_guarded(guarded);
                }
                return;
            }
            Action::UnsavedSave => {
                if let Some(guarded) = self.awaiting_confirmation.take() {
                    if self.save(false) {
                        self.run_guarded(guarded);
                    }
                }
                return;
            }
            _ if self.is_blocked() => return,
            _ => {}
        }

        // Everything below changes or reads the document as a whole.
        let view_only = matches!(action, Action::FitView | Action::ActualSize | Action::ResetRotation);
        if !view_only {
            self.finish_stroke();
        }
        let center = vec2(canvas_rect.center().x, canvas_rect.center().y);
        let pixels: &mut dyn PixelStore = self.renderer.pixels();
        let changed = match action {
            Action::FitView => {
                self.fit(canvas_rect);
                false
            }
            Action::ActualSize => {
                self.view.set_zoom_about(center, 1.0);
                false
            }
            Action::ResetRotation => {
                self.view.rotate_about(center, -self.view.rotation);
                false
            }
            Action::Undo => self.history.undo(&mut self.document, pixels),
            Action::Redo => self.history.redo(&mut self.document, pixels),
            Action::AddLayer => {
                // Directly above the active layer.
                let index = self.document.index_of(self.document.active()).map_or(0, |i| i + 1);
                let layer = self.document.new_layer();
                let edit = Edit::InsertLayer { index, layer, pixels: None };
                self.history.perform(edit, &mut self.document, pixels)
            }
            Action::DeleteLayer(id) => {
                self.history.perform(Edit::RemoveLayer { id }, &mut self.document, pixels)
            }
            Action::MoveLayer { id, up } => {
                let target = self.document.index_of(id).and_then(|index| {
                    if up {
                        (index + 1 < self.document.layers().len()).then_some(index + 1)
                    } else {
                        index.checked_sub(1)
                    }
                });
                target.is_some_and(|to| {
                    self.history.perform(Edit::MoveLayer { id, to }, &mut self.document, pixels)
                })
            }
            Action::SelectLayer(id) => {
                self.document.set_active(id);
                false
            }
            Action::SetLayerProps { id, props, finished } => {
                let differs = self.document.layer(id).is_some_and(|layer| layer.props != props);
                let changed = differs && self.history.perform_props(id, props, &mut self.document);
                if finished {
                    self.history.close_props();
                }
                changed
            }
            Action::New { width, height } => {
                self.guarded(Guarded::New { width, height });
                false
            }
            Action::Open => {
                self.guarded(Guarded::Open);
                false
            }
            Action::Save => {
                self.save(false);
                false
            }
            Action::SaveAs => {
                self.save(true);
                false
            }
            Action::Export => {
                self.export();
                false
            }
            Action::DismissError | Action::UnsavedCancel | Action::UnsavedDiscard | Action::UnsavedSave => {
                false
            }
        };
        if changed {
            self.modified = true;
        }
        self.composite_dirty = true;
    }

    /// Runs `guarded` now, or asks first if there are unsaved changes.
    fn guarded(&mut self, guarded: Guarded) {
        self.finish_stroke();
        if self.modified {
            self.awaiting_confirmation = Some(guarded);
        } else {
            self.run_guarded(guarded);
        }
    }

    fn run_guarded(&mut self, guarded: Guarded) {
        let result = match guarded {
            Guarded::Close => {
                self.exit_requested = true;
                Ok(())
            }
            Guarded::New { width, height } => self.new_document(width, height),
            Guarded::Open => self.open(),
        };
        self.report(result);
    }

    fn report(&mut self, result: anyhow::Result<()>) -> bool {
        match result {
            Ok(()) => true,
            Err(error) => {
                tracing::error!("{error:#}");
                self.error = Some(format!("{error:#}"));
                false
            }
        }
    }

    // --- Documents and files ---------------------------------------------

    /// Replaces the open document. `layers` holds premultiplied RGBA8 pixels
    /// for some or all of its layers.
    fn set_document(&mut self, document: Document, layers: &[(LayerId, Vec<u8>)], path: Option<PathBuf>) {
        let mut renderer = CanvasRenderer::new(&self.device, &self.queue, self.target_format, &document);
        let bounds = renderer.bounds();
        for (id, pixels) in layers {
            renderer.pixels().write_rect(*id, bounds, pixels);
        }
        self.document = document;
        self.renderer = renderer;
        self.history = History::new(HISTORY_BUDGET_BYTES);
        self.path = path;
        self.modified = false;
        self.stroke = None;
        self.fit_pending = true;
        self.composite_dirty = true;
    }

    fn new_document(&mut self, width: u32, height: u32) -> anyhow::Result<()> {
        let max = CanvasRenderer::max_side(&self.device);
        anyhow::ensure!(
            width <= max && height <= max,
            "El lienzo de {width} × {height} px supera el máximo de {max} px de este equipo."
        );
        let document = Document::new(width, height)?;
        self.set_document(document, &[], None);
        Ok(())
    }

    fn open(&mut self) -> anyhow::Result<()> {
        let mut extensions = vec![dp_io::DOCUMENT_EXTENSION];
        extensions.extend(IMAGE_EXTENSIONS);
        let Some(path) = rfd::FileDialog::new().add_filter("DrawPaint e imágenes", &extensions).pick_file()
        else {
            return Ok(());
        };
        self.open_path(&path)
    }

    /// Opens the file given on the command line, reporting failure in the UI.
    pub fn open_at_startup(&mut self, path: &Path) {
        let result = self.open_path(path);
        self.report(result);
    }

    fn open_path(&mut self, path: &Path) -> anyhow::Result<()> {
        let max = CanvasRenderer::max_side(&self.device);
        let extension = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        if extension.as_deref() == Some(dp_io::DOCUMENT_EXTENSION) {
            let loaded = dp_io::load_document(path, max)
                .with_context(|| format!("No se pudo abrir {}", path.display()))?;
            self.set_document(loaded.document, &loaded.layers, Some(path.to_owned()));
        } else {
            let image = dp_io::import_image(path, max)
                .with_context(|| format!("No se pudo abrir {}", path.display()))?;
            let document = Document::new(image.width, image.height)?;
            let layer = document.active();
            // An imported image has no document file yet: saving asks for one.
            self.set_document(document, &[(layer, image.pixels)], None);
        }
        Ok(())
    }

    /// Saves the document, asking for a path if it has none or `ask` is set.
    /// Returns whether it was saved.
    fn save(&mut self, ask: bool) -> bool {
        self.finish_stroke();
        let path = match (&self.path, ask) {
            (Some(path), false) => path.clone(),
            _ => {
                let dialog = rfd::FileDialog::new()
                    .add_filter("Documento de DrawPaint", &[dp_io::DOCUMENT_EXTENSION])
                    .set_file_name(self.file_name().unwrap_or("Sin título.dpaint"));
                match dialog.save_file() {
                    Some(path) => with_default_extension(path, dp_io::DOCUMENT_EXTENSION),
                    None => return false,
                }
            }
        };
        let result = self.save_to(&path);
        let saved = self.report(result);
        if saved {
            self.path = Some(path);
            self.modified = false;
        }
        saved
    }

    fn save_to(&mut self, path: &Path) -> anyhow::Result<()> {
        let merged = self.renderer.flatten(&self.document);
        let bounds = self.renderer.bounds();
        let pixels = self.renderer.pixels();
        dp_io::save_document(path, &self.document, |id| pixels.read_rect(id, bounds), &merged)
            .with_context(|| format!("No se pudo guardar {}", path.display()))
    }

    fn export(&mut self) {
        self.finish_stroke();
        let stem = self.path.as_deref().and_then(Path::file_stem).and_then(|s| s.to_str());
        let dialog = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .add_filter("JPEG", &["jpg", "jpeg"])
            .set_file_name(format!("{}.png", stem.unwrap_or("Sin título")));
        let Some(path) = dialog.save_file() else { return };
        let result = self.export_to(&with_default_extension(path, "png"));
        self.report(result);
    }

    fn export_to(&mut self, path: &Path) -> anyhow::Result<()> {
        let bounds = self.renderer.bounds();
        let flat = self.renderer.flatten(&self.document);
        dp_io::export_image(path, bounds.width, bounds.height, &flat)
            .with_context(|| format!("No se pudo exportar {}", path.display()))
    }

    // --- Frame ------------------------------------------------------------

    fn fit(&mut self, canvas_rect: egui::Rect) {
        let (min, size) = (canvas_rect.min, canvas_rect.size());
        self.view = ViewTransform::fit(self.document.size(), vec2(min.x, min.y), vec2(size.x, size.y));
    }

    /// Fits a new document to the canvas area once that area is known.
    pub fn fit_if_pending(&mut self, canvas_rect: egui::Rect) {
        if self.fit_pending && canvas_rect.is_positive() {
            self.fit(canvas_rect);
            self.fit_pending = false;
        }
    }

    /// Brings the composite up to date outside the normal frame.
    fn refresh_composite(&mut self) {
        if self.composite_dirty {
            let mut encoder = self.device.create_command_encoder(&Default::default());
            self.composite(&mut encoder);
            self.queue.submit([encoder.finish()]);
        }
    }

    fn composite(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let stroke = self.stroke.as_ref().map(|stroke| (stroke.layer, stroke.style));
        self.renderer.composite(encoder, &self.document, stroke);
        self.composite_dirty = false;
    }

    /// Records this frame's canvas work. `target_size` is the window size in
    /// physical pixels.
    pub fn prepare_frame(&mut self, encoder: &mut wgpu::CommandEncoder, target_size: Vec2) {
        if self.composite_dirty {
            self.composite(encoder);
        }
        self.renderer.set_view(self.view.canvas_to_screen(), target_size);
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.renderer.draw(pass);
    }
}

fn with_default_extension(mut path: PathBuf, extension: &str) -> PathBuf {
    if path.extension().is_none() {
        path.set_extension(extension);
    }
    path
}

#[cfg(test)]
mod tests {
    use dp_core::BlendMode;
    use dp_input::{DeviceKind, Phase, PointerSample};

    use super::*;

    const CANVAS_AREA: egui::Rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));

    /// Returns `None` when the machine has no usable graphics adapter; with
    /// `DP_REQUIRE_GPU=1` that is a failure instead.
    fn session() -> Option<Session> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = match pollster::block_on(instance.request_adapter(&Default::default())) {
            Ok(adapter) => adapter,
            Err(error) => {
                assert!(std::env::var_os("DP_REQUIRE_GPU").is_none(), "no graphics adapter: {error}");
                eprintln!("skipping: no graphics adapter ({error})");
                return None;
            }
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let mut session = Session::new(&device, &queue, dp_render::CANVAS_FORMAT).unwrap();
        // A small canvas shown 1:1 keeps the test fast and coordinates simple.
        session.new_document(64, 48).unwrap();
        session.view = ViewTransform::default();
        session.fit_pending = false;
        Some(session)
    }

    fn sample(phase: Phase, x: f32, y: f32) -> PointerSample {
        let pressure = if phase == Phase::Up { 0.0 } else { 1.0 };
        PointerSample {
            device: DeviceKind::Pen,
            phase,
            pos: vec2(x, y),
            pressure,
            tilt: Vec2::ZERO,
            twist: 0.0,
            time: 0.0,
        }
    }

    /// Draws a horizontal line at `y` with the current brush.
    fn draw_line(session: &mut Session, ui: &mut UiState, y: f32) {
        session.pointer(&sample(Phase::Down, 10.0, y), true, false, ui);
        session.pointer(&sample(Phase::Move, 50.0, y), true, false, ui);
        session.pointer(&sample(Phase::Up, 50.0, y), true, false, ui);
    }

    fn flat(session: &mut Session) -> Vec<u8> {
        session.renderer.flatten(&session.document)
    }

    fn pixel(pixels: &[u8], x: usize, y: usize) -> [u8; 4] {
        pixels[(y * 64 + x) * 4..][..4].try_into().unwrap()
    }

    fn hard_brush(ui: &mut UiState, color: [f32; 3]) {
        ui.brush = dp_brush::BrushParams {
            size: 8.0,
            hardness: 1.0,
            pressure_affects_size: false,
            ..Default::default()
        };
        ui.color = color;
    }

    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    #[test]
    fn paint_layers_undo_and_redo() {
        let Some(mut session) = session() else { return };
        let mut ui = UiState::default();
        assert!(!session.modified);

        hard_brush(&mut ui, [1.0, 0.0, 0.0]);
        draw_line(&mut session, &mut ui, 12.0);
        assert!(session.modified);
        assert_eq!(pixel(&flat(&mut session), 30, 12), RED);

        // A second layer on top, painted blue across the same pixels.
        session.apply(Action::AddLayer, CANVAS_AREA);
        assert_eq!(session.document.layers().len(), 2);
        hard_brush(&mut ui, [0.0, 0.0, 1.0]);
        draw_line(&mut session, &mut ui, 12.0);
        assert_eq!(pixel(&flat(&mut session), 30, 12), BLUE);

        // Hiding the top layer reveals the red line below.
        let top = session.document.active();
        let hidden = dp_core::LayerProps { visible: false, ..session.document.active_layer().props.clone() };
        session.apply(Action::SetLayerProps { id: top, props: hidden, finished: true }, CANVAS_AREA);
        assert_eq!(pixel(&flat(&mut session), 30, 12), RED);

        // Undo walks back: visibility, blue stroke, new layer, red stroke.
        session.apply(Action::Undo, CANVAS_AREA);
        assert_eq!(pixel(&flat(&mut session), 30, 12), BLUE);
        session.apply(Action::Undo, CANVAS_AREA);
        assert_eq!(pixel(&flat(&mut session), 30, 12), RED);
        session.apply(Action::Undo, CANVAS_AREA);
        assert_eq!(session.document.layers().len(), 1);
        session.apply(Action::Undo, CANVAS_AREA);
        assert_eq!(pixel(&flat(&mut session), 30, 12), WHITE);
        assert!(!session.history.can_undo());

        for _ in 0..4 {
            session.apply(Action::Redo, CANVAS_AREA);
        }
        assert_eq!(session.document.layers().len(), 2);
        assert_eq!(pixel(&flat(&mut session), 30, 12), RED, "top layer is hidden again");
    }

    #[test]
    fn eraser_and_eyedropper() {
        let Some(mut session) = session() else { return };
        let mut ui = UiState::default();
        hard_brush(&mut ui, [1.0, 0.0, 0.0]);
        draw_line(&mut session, &mut ui, 20.0);

        // Alt + click picks the colour under the pointer without painting.
        ui.color = [0.0, 1.0, 0.0];
        session.pointer(&sample(Phase::Down, 30.0, 20.0), true, true, &mut ui);
        session.pointer(&sample(Phase::Up, 30.0, 20.0), true, true, &mut ui);
        assert_eq!(ui.color, [1.0, 0.0, 0.0]);
        assert_eq!(pixel(&flat(&mut session), 30, 20), RED);

        // The eraser end of the pen erases whatever the brush settings say.
        let mut down = sample(Phase::Down, 10.0, 20.0);
        down.device = DeviceKind::Eraser;
        session.pointer(&down, true, false, &mut ui);
        session.pointer(&sample(Phase::Move, 50.0, 20.0), true, false, &mut ui);
        session.pointer(&sample(Phase::Up, 50.0, 20.0), true, false, &mut ui);
        assert_eq!(pixel(&flat(&mut session), 30, 20), WHITE);
    }

    #[test]
    fn hidden_layer_refuses_paint() {
        let Some(mut session) = session() else { return };
        let mut ui = UiState::default();
        let id = session.document.active();
        let hidden = dp_core::LayerProps { visible: false, ..session.document.active_layer().props.clone() };
        session.apply(Action::SetLayerProps { id, props: hidden, finished: true }, CANVAS_AREA);
        draw_line(&mut session, &mut ui, 12.0);
        assert!(session.error.is_some());
        assert!(!session.is_painting());
        // While the message is up nothing else happens until it is dismissed.
        assert!(session.is_blocked());
        session.apply(Action::AddLayer, CANVAS_AREA);
        assert_eq!(session.document.layers().len(), 1);
        session.apply(Action::DismissError, CANVAS_AREA);
        assert!(!session.is_blocked());
    }

    #[test]
    fn save_reopen_and_export() {
        let Some(mut session) = session() else { return };
        let mut ui = UiState::default();
        hard_brush(&mut ui, [1.0, 0.0, 0.0]);
        draw_line(&mut session, &mut ui, 12.0);
        session.apply(Action::AddLayer, CANVAS_AREA);
        hard_brush(&mut ui, [0.0, 0.0, 1.0]);
        draw_line(&mut session, &mut ui, 30.0);
        let top = session.document.active();
        let props = dp_core::LayerProps {
            name: "Tinta".to_owned(),
            opacity: 0.5,
            blend: BlendMode::Multiply,
            ..session.document.active_layer().props.clone()
        };
        session.apply(Action::SetLayerProps { id: top, props, finished: true }, CANVAS_AREA);
        let document = session.document.clone();
        let image = flat(&mut session);

        let dir = std::env::temp_dir().join(format!("drawpaint-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.dpaint");
        session.save_to(&file).unwrap();
        let png = dir.join("test.png");
        session.export_to(&png).unwrap();

        // Start over, then load the saved document back.
        session.new_document(16, 16).unwrap();
        assert_ne!(session.document, document);
        session.open_path(&file).unwrap();
        assert_eq!(session.document, document);
        assert_eq!(flat(&mut session), image);
        assert!(!session.modified && !session.history.can_undo());
        assert_eq!(session.file_name(), Some("test.dpaint"));

        // The exported PNG opens as a single-layer document with the same picture.
        session.open_path(&png).unwrap();
        assert_eq!(session.document.layers().len(), 1);
        assert_eq!(flat(&mut session), image);
        assert_eq!(session.file_name(), None);

        let missing = session.open_path(&dir.join("missing.dpaint"));
        assert!(missing.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unsaved_changes_are_guarded() {
        let Some(mut session) = session() else { return };
        let mut ui = UiState::default();
        draw_line(&mut session, &mut ui, 12.0);

        // Closing with unsaved work asks first; cancelling keeps everything.
        session.request_close();
        assert!(session.is_blocked() && !session.exit_requested());
        session.apply(Action::UnsavedCancel, CANVAS_AREA);
        assert!(!session.is_blocked() && !session.exit_requested());
        assert!(session.modified);

        // A new canvas discards the work only after confirmation.
        session.apply(Action::New { width: 20, height: 10 }, CANVAS_AREA);
        assert_eq!(session.document.width(), 64, "nothing is replaced before the answer");
        session.apply(Action::UnsavedDiscard, CANVAS_AREA);
        assert_eq!((session.document.width(), session.document.height()), (20, 10));
        assert!(!session.modified);

        // With nothing to lose, closing is immediate.
        session.request_close();
        assert!(session.exit_requested());
    }
}
