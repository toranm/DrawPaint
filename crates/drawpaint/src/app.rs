//! Event loop glue between the window, the UI, input and rendering.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use dp_brush::{Dab, StrokeBuilder};
use dp_core::{Document, ViewTransform};
use dp_input::{DeviceKind, NativePen, Phase, PointerSample};
use dp_render::{Canvas, CanvasPresenter, DabRenderer};
use dp_ui::{Action, PointerInfo, UiState, ViewInfo};
use glam::{vec2, Vec2};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;
use crate::pointer::PointerInput;

const DEFAULT_CANVAS_SIDE: u32 = 2048;
/// Premultiplied colour of a new canvas, and what the eraser paints with
/// until real transparency arrives with layers in phase 1.
const PAPER: [f32; 4] = [1.0; 4];
const WORKSPACE_BACKGROUND: wgpu::Color = wgpu::Color { r: 0.16, g: 0.16, b: 0.17, a: 1.0 };
const ZOOM_PER_WHEEL_LINE: f32 = 1.15;
const ROTATION_PER_WHEEL_LINE: f32 = std::f32::consts::PI / 36.0;
/// Pixels of touchpad scroll treated as one wheel line.
const PIXELS_PER_WHEEL_LINE: f32 = 40.0;

pub fn run() -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("create event loop")?;
    let mut app = App::default();
    event_loop.run_app(&mut app).context("run event loop")?;
    match app.error.take() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[derive(Default)]
struct App {
    state: Option<State>,
    error: Option<anyhow::Error>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match State::new(event_loop) {
            Ok(state) => self.state = Some(state),
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let Some(state) = &mut self.state {
            state.window_event(event_loop, event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.state {
            state.about_to_wait(event_loop);
        }
    }
}

struct Stroke {
    builder: StrokeBuilder,
    color: [f32; 3],
}

enum Drag {
    None,
    Pan { last: Vec2 },
}

struct State {
    window: Arc<Window>,
    gpu: Gpu,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,

    document: Document,
    canvas: Canvas,
    dabs: DabRenderer,
    presenter: CanvasPresenter,
    view: ViewTransform,
    /// The view is fitted once the UI has reported the canvas area.
    fit_pending: bool,
    clear_pending: bool,
    /// Canvas area in physical pixels, from the last UI pass.
    canvas_rect: egui::Rect,

    ui: UiState,
    pointer: PointerInput,
    samples: Vec<PointerSample>,
    dab_scratch: Vec<Dab>,
    stroke: Option<Stroke>,
    drag: Drag,
    modifiers: ModifiersState,
    space_held: bool,
    repaint_at: Option<Instant>,
}

impl State {
    fn new(event_loop: &ActiveEventLoop) -> anyhow::Result<Self> {
        let attributes = Window::default_attributes()
            .with_title("DrawPaint")
            .with_inner_size(LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(event_loop.create_window(attributes).context("create window")?);
        let gpu = Gpu::new(window.clone(), event_loop.owned_display_handle())?;

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(&gpu.device, gpu.config.format, egui_wgpu::RendererOptions::default());

        let side = DEFAULT_CANVAS_SIDE.min(Canvas::max_side(&gpu.device));
        let document = Document::new(side, side);
        let canvas = Canvas::new(&gpu.device, document.width, document.height);
        let dabs = DabRenderer::new(&gpu.device);
        let presenter = CanvasPresenter::new(&gpu.device, gpu.config.format, &canvas);

        let native_pen = NativePen::install(&window);
        tracing::info!(native_pen = native_pen.is_some(), "pointer input ready");

        Ok(Self {
            window,
            gpu,
            egui_ctx,
            egui_state,
            egui_renderer,
            document,
            canvas,
            dabs,
            presenter,
            view: ViewTransform::default(),
            fit_pending: true,
            clear_pending: true,
            canvas_rect: egui::Rect::NOTHING,
            ui: UiState::default(),
            pointer: PointerInput::new(native_pen),
            samples: Vec::new(),
            dab_scratch: Vec::new(),
            stroke: None,
            drag: Drag::None,
            modifiers: ModifiersState::empty(),
            space_held: false,
            repaint_at: None,
        })
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        // Pen samples are captured by the native hook before winit sees the
        // message, so they are always ahead of the event handled here.
        self.pointer.poll_native(&mut self.samples);

        let response = self.egui_state.on_window_event(&self.window, &event);
        if response.repaint {
            self.window.request_redraw();
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.gpu.resize(size.width, size.height);
                self.window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if event.logical_key == Key::Named(NamedKey::Space) {
                    self.space_held = pressed;
                } else if pressed && !event.repeat && !self.egui_ctx.egui_wants_keyboard_input() {
                    self.shortcut(&event.logical_key);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let pos = vec2(position.x as f32, position.y as f32);
                if let Drag::Pan { last } = &mut self.drag {
                    self.view.pan(pos - *last);
                    *last = pos;
                    self.window.request_redraw();
                }
                self.pointer.cursor_moved(pos, &mut self.samples);
            }
            WindowEvent::MouseInput { button, state, .. } => {
                let pressed = state == ElementState::Pressed;
                let pans = button == MouseButton::Middle || (button == MouseButton::Left && self.space_held);
                if pressed && pans && self.stroke.is_none() && self.is_over_canvas(self.pointer.cursor()) {
                    self.drag = Drag::Pan { last: self.pointer.cursor() };
                } else if !pressed && matches!(self.drag, Drag::Pan { .. }) {
                    self.drag = Drag::None;
                } else {
                    self.pointer.mouse_button(button, state, &mut self.samples);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (x, y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(p) => {
                        (p.x as f32 / PIXELS_PER_WHEEL_LINE, p.y as f32 / PIXELS_PER_WHEEL_LINE)
                    }
                };
                // Some platforms turn Shift + wheel into horizontal scrolling.
                let lines = if y != 0.0 { y } else { x };
                let anchor = self.pointer.cursor();
                if self.is_over_canvas(anchor) {
                    if self.modifiers.shift_key() {
                        self.view.rotate_about(anchor, lines * ROTATION_PER_WHEEL_LINE);
                    } else {
                        self.view.zoom_about(anchor, ZOOM_PER_WHEEL_LINE.powf(lines));
                    }
                    self.window.request_redraw();
                }
            }
            WindowEvent::Touch(touch) => self.pointer.touch(&touch, &mut self.samples),
            _ => {}
        }
        self.process_samples();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.pointer.poll_native(&mut self.samples);
        self.process_samples();
        match self.repaint_at {
            Some(at) if at <= Instant::now() => {
                self.repaint_at = None;
                self.window.request_redraw();
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn shortcut(&mut self, key: &Key) {
        if !self.modifiers.control_key() {
            return;
        }
        match key {
            Key::Character(c) if c == "0" => self.apply(Action::FitView),
            Key::Character(c) if c == "1" => self.apply(Action::ActualSize),
            _ => return,
        }
        self.window.request_redraw();
    }

    fn canvas_center(&self) -> Vec2 {
        let c = self.canvas_rect.center();
        vec2(c.x, c.y)
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::FitView => {
                let (min, size) = (self.canvas_rect.min, self.canvas_rect.size());
                self.view =
                    ViewTransform::fit(self.document.size(), vec2(min.x, min.y), vec2(size.x, size.y));
            }
            Action::ActualSize => self.view.set_zoom_about(self.canvas_center(), 1.0),
            Action::ResetRotation => self.view.rotate_about(self.canvas_center(), -self.view.rotation),
            Action::ClearCanvas => {
                self.stroke = None;
                self.clear_pending = true;
            }
        }
    }

    /// Whether a window position (physical pixels) is on the canvas area and
    /// not covered by a popup or other floating UI.
    fn is_over_canvas(&self, pos: Vec2) -> bool {
        let pos = egui::pos2(pos.x, pos.y);
        if !self.canvas_rect.contains(pos) {
            return false;
        }
        let points = pos / self.egui_ctx.pixels_per_point();
        self.egui_ctx.layer_id_at(points).is_none_or(|layer| layer.order == egui::Order::Background)
    }

    /// Feeds queued pointer samples to the brush engine.
    fn process_samples(&mut self) {
        if self.samples.is_empty() {
            return;
        }
        let samples = std::mem::take(&mut self.samples);
        for sample in &samples {
            let pos = self.view.to_canvas(sample.pos);
            match sample.phase {
                Phase::Down => {
                    let blocked = matches!(self.drag, Drag::Pan { .. }) || self.space_held;
                    if !blocked && self.is_over_canvas(sample.pos) {
                        let color = match sample.device {
                            DeviceKind::Eraser => [PAPER[0], PAPER[1], PAPER[2]],
                            _ => self.ui.color,
                        };
                        let builder =
                            StrokeBuilder::begin(self.ui.brush, pos, sample.pressure, &mut self.dab_scratch);
                        self.stroke = Some(Stroke { builder, color });
                    }
                }
                Phase::Move => {
                    if let Some(stroke) = &mut self.stroke {
                        stroke.builder.extend(pos, sample.pressure, &mut self.dab_scratch);
                    }
                }
                Phase::Up => {
                    // The lift-off sample carries no pressure; the stroke
                    // ends where the last contact sample left it.
                    self.flush_dab_scratch();
                    self.stroke = None;
                }
                Phase::Hover => {}
            }
        }
        self.flush_dab_scratch();
        // Reuse the allocation for the next batch.
        self.samples = samples;
        self.samples.clear();
        // The pointer readout changes with every sample.
        self.window.request_redraw();
    }

    fn flush_dab_scratch(&mut self) {
        if let Some(stroke) = &self.stroke {
            self.dabs.queue(&self.dab_scratch, stroke.color);
        }
        self.dab_scratch.clear();
    }

    fn redraw(&mut self) {
        let pixels_per_point = egui_winit::pixels_per_point(&self.egui_ctx, &self.window);
        let view_info = ViewInfo {
            zoom: self.view.zoom,
            rotation_degrees: self.view.rotation.to_degrees(),
            canvas_size: [self.document.width, self.document.height],
        };
        let last = self.pointer.last_sample();
        let pointer_info = PointerInfo {
            native_pen_backend: self.pointer.has_native_pen(),
            device: last.map(|s| s.device),
            pressure: last.map_or(0.0, |s| s.pressure),
            tilt_degrees: last.map_or([0.0; 2], |s| s.tilt.to_array()),
            samples_per_second: self.pointer.samples_per_second(),
        };

        let raw_input = self.egui_state.take_egui_input(&self.window);
        let mut ui_output = None;
        let full_output = self.egui_ctx.run_ui(raw_input, |ui| {
            ui_output = Some(dp_ui::show(ui, &mut self.ui, view_info, pointer_info));
        });
        self.egui_state.handle_platform_output(&self.window, full_output.platform_output);

        if let Some(output) = ui_output {
            self.canvas_rect = output.canvas_rect * pixels_per_point;
            if std::mem::take(&mut self.fit_pending) {
                self.apply(Action::FitView);
            }
            for action in output.actions {
                self.apply(action);
            }
        }

        let paint_jobs = self.egui_ctx.tessellate(full_output.shapes, pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.gpu.config.width, self.gpu.config.height],
            pixels_per_point,
        };
        let Gpu { device, queue, .. } = &self.gpu;
        // egui requires every texture delta to be consumed, not just read.
        let mut textures = full_output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui_renderer.update_texture(device, queue, id, delta);
            }
        }

        let mut encoder = device.create_command_encoder(&Default::default());
        if std::mem::take(&mut self.clear_pending) {
            self.canvas.clear(&mut encoder, PAPER);
        }
        self.dabs.flush(device, queue, &mut encoder, &self.canvas);
        self.presenter.set_view(queue, &self.canvas, self.view.canvas_to_screen(), self.gpu.size());
        let egui_commands =
            self.egui_renderer.update_buffers(device, queue, &mut encoder, &paint_jobs, &screen);

        use wgpu::CurrentSurfaceTexture as Frame;
        let frame = match self.gpu.surface.get_current_texture() {
            Frame::Success(frame) => Some(frame),
            Frame::Suboptimal(frame) => {
                // Still drawable; pick up the new surface state next frame.
                self.window.request_redraw();
                Some(frame)
            }
            Frame::Outdated | Frame::Lost => {
                self.gpu.reconfigure();
                self.window.request_redraw();
                None
            }
            Frame::Timeout | Frame::Occluded => None,
            Frame::Validation => {
                tracing::error!("surface validation error while acquiring a frame");
                None
            }
        };

        if let Some(frame) = &frame {
            let target = frame.texture.create_view(&Default::default());
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("window"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(WORKSPACE_BACKGROUND),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.presenter.draw(&mut pass);
            self.egui_renderer.render(&mut pass, &paint_jobs, &screen);
        }

        // Canvas edits are submitted even when no frame could be acquired.
        queue.submit(egui_commands.into_iter().chain([encoder.finish()]));
        if let Some(frame) = frame {
            self.window.pre_present_notify();
            queue.present(frame);
        }
        for id in textures.free.drain() {
            self.egui_renderer.free_texture(&id);
        }

        // Repaint when egui asks (animations, tooltips) and while the
        // samples-per-second readout is still decaying.
        let mut delay = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::MAX, |viewport| viewport.repaint_delay);
        if self.pointer.rate_is_settling() {
            delay = delay.min(Duration::from_millis(250));
        }
        self.repaint_at = Instant::now().checked_add(delay);
    }
}
