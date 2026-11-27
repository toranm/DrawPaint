//! Event loop glue between the window, the UI, input and the session.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use dp_input::{NativePen, PointerSample};
use dp_ui::{PointerInfo, UiState};
use glam::{vec2, Vec2};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;
use crate::pointer::PointerInput;
use crate::session::Session;

const WORKSPACE_BACKGROUND: wgpu::Color = wgpu::Color { r: 0.16, g: 0.16, b: 0.17, a: 1.0 };
const ZOOM_PER_WHEEL_LINE: f32 = 1.15;
const ROTATION_PER_WHEEL_LINE: f32 = std::f32::consts::PI / 36.0;
/// Pixels of touchpad scroll treated as one wheel line.
const PIXELS_PER_WHEEL_LINE: f32 = 40.0;
const BRUSH_SIZE_STEP: f32 = 1.15;

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
            state.window_event(event);
            if state.session.exit_requested() {
                event_loop.exit();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.state {
            state.about_to_wait(event_loop);
        }
    }
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

    session: Session,
    ui: UiState,
    /// Canvas area in physical pixels, from the last UI pass.
    canvas_rect: egui::Rect,
    /// Window title last sent to the system.
    title: String,

    pointer: PointerInput,
    samples: Vec<PointerSample>,
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

        let mut session = Session::new(&gpu.device, &gpu.queue, gpu.config.format)?;
        // A document or image passed on the command line opens at startup.
        if let Some(path) = std::env::args_os().nth(1) {
            session.open_at_startup(std::path::Path::new(&path));
        }
        let native_pen = NativePen::install(&window);
        tracing::info!(native_pen = native_pen.is_some(), "pointer input ready");

        Ok(Self {
            window,
            gpu,
            egui_ctx,
            egui_state,
            egui_renderer,
            session,
            ui: UiState::default(),
            canvas_rect: egui::Rect::NOTHING,
            title: String::new(),
            pointer: PointerInput::new(native_pen),
            samples: Vec::new(),
            drag: Drag::None,
            modifiers: ModifiersState::empty(),
            space_held: false,
            repaint_at: None,
        })
    }

    fn window_event(&mut self, event: WindowEvent) {
        // Pen samples are captured by the native hook before winit sees the
        // message, so they are always ahead of the event handled here.
        self.pointer.poll_native(&mut self.samples);

        let response = self.egui_state.on_window_event(&self.window, &event);
        if response.repaint {
            self.window.request_redraw();
        }

        match event {
            WindowEvent::CloseRequested => {
                self.session.request_close();
                self.window.request_redraw();
            }
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
                } else if pressed && !self.egui_ctx.egui_wants_keyboard_input() && !self.session.is_blocked()
                {
                    self.shortcut(&event.logical_key, event.repeat);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let pos = vec2(position.x as f32, position.y as f32);
                if let Drag::Pan { last } = &mut self.drag {
                    self.session.view.pan(pos - *last);
                    *last = pos;
                    self.window.request_redraw();
                }
                self.pointer.cursor_moved(pos, &mut self.samples);
            }
            WindowEvent::MouseInput { button, state, .. } => {
                let pressed = state == ElementState::Pressed;
                let pans = button == MouseButton::Middle || (button == MouseButton::Left && self.space_held);
                let free = !self.session.is_painting() && self.is_over_canvas(self.pointer.cursor());
                if pressed && pans && free {
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
                        self.session.view.rotate_about(anchor, lines * ROTATION_PER_WHEEL_LINE);
                    } else {
                        self.session.view.zoom_about(anchor, ZOOM_PER_WHEEL_LINE.powf(lines));
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

    fn shortcut(&mut self, key: &Key, repeat: bool) {
        use dp_ui::Action;
        let Key::Character(text) = key else { return };
        let text = text.to_lowercase();
        let (ctrl, shift) = (self.modifiers.control_key(), self.modifiers.shift_key());
        let action = match (ctrl, shift, text.as_str()) {
            (true, false, "z") => Some(Action::Undo),
            (true, false, "y") | (true, true, "z") => Some(Action::Redo),
            _ if repeat => None,
            (true, false, "s") => Some(Action::Save),
            (true, true, "s") => Some(Action::SaveAs),
            (true, false, "o") => Some(Action::Open),
            (true, false, "e") => Some(Action::Export),
            (true, false, "0") => Some(Action::FitView),
            (true, false, "1") => Some(Action::ActualSize),
            (true, false, "n") => {
                let document = self.session.document();
                self.ui.new_canvas = Some([document.width(), document.height()]);
                None
            }
            _ => None,
        };
        if let Some(action) = action {
            self.session.apply(action, self.canvas_rect);
        } else if !ctrl {
            let presets = dp_brush::presets();
            match text.as_str() {
                "[" => self.ui.brush.size = (self.ui.brush.size / BRUSH_SIZE_STEP).max(1.0),
                "]" => self.ui.brush.size = (self.ui.brush.size * BRUSH_SIZE_STEP).min(500.0),
                "b" if !repeat => self.ui.select_preset(0),
                "e" if !repeat => {
                    if let Some(eraser) = presets.iter().position(|p| p.params.erase) {
                        self.ui.select_preset(eraser);
                    }
                }
                _ => return,
            }
        }
        self.window.request_redraw();
    }

    /// Whether a window position (physical pixels) is on the canvas area and
    /// not covered by a popup or other floating UI.
    fn is_over_canvas(&self, pos: Vec2) -> bool {
        let pos = egui::pos2(pos.x, pos.y);
        if !self.canvas_rect.contains(pos) || self.session.is_blocked() {
            return false;
        }
        let points = pos / self.egui_ctx.pixels_per_point();
        self.egui_ctx.layer_id_at(points).is_none_or(|layer| layer.order == egui::Order::Background)
    }

    /// Feeds queued pointer samples to the session.
    fn process_samples(&mut self) {
        if self.samples.is_empty() {
            return;
        }
        let samples = std::mem::take(&mut self.samples);
        let panning = matches!(self.drag, Drag::Pan { .. }) || self.space_held;
        for sample in &samples {
            let may_start = !panning && self.is_over_canvas(sample.pos);
            let pick_color = self.modifiers.alt_key();
            self.session.pointer(sample, may_start, pick_color, &mut self.ui);
        }
        // Reuse the allocation for the next batch.
        self.samples = samples;
        self.samples.clear();
        // The pointer readout and brush outline change with every sample.
        self.window.request_redraw();
    }

    fn redraw(&mut self) {
        let pixels_per_point = egui_winit::pixels_per_point(&self.egui_ctx, &self.window);
        let last = self.pointer.last_sample();
        let pointer_info = PointerInfo {
            native_pen_backend: self.pointer.has_native_pen(),
            device: last.map(|s| s.device),
            pressure: last.map_or(0.0, |s| s.pressure),
            tilt_degrees: last.map_or([0.0; 2], |s| s.tilt.to_array()),
            samples_per_second: self.pointer.samples_per_second(),
        };
        let max_side = dp_render::CanvasRenderer::max_side(&self.gpu.device);
        let cursor = self.pointer.cursor();
        let show_outline = self.is_over_canvas(cursor) && !matches!(self.drag, Drag::Pan { .. });

        let raw_input = self.egui_state.take_egui_input(&self.window);
        let mut ui_output = None;
        let full_output = self.egui_ctx.run_ui(raw_input, |ui| {
            let frame = self.session.frame(pointer_info, max_side);
            ui_output = Some(dp_ui::show(ui, &mut self.ui, &frame));
            if show_outline {
                // Brush outline at the cursor, at its on-canvas size.
                let radius = self.ui.brush.size * 0.5 * self.session.view.zoom / pixels_per_point;
                let center = egui::pos2(cursor.x, cursor.y) / pixels_per_point;
                let painter = ui.ctx().layer_painter(egui::LayerId::background());
                painter.circle_stroke(center, radius + 1.0, (1.0, egui::Color32::from_black_alpha(160)));
                painter.circle_stroke(center, radius, (1.0, egui::Color32::from_white_alpha(220)));
            }
        });
        self.egui_state.handle_platform_output(&self.window, full_output.platform_output);

        if let Some(output) = ui_output {
            self.canvas_rect = output.canvas_rect * pixels_per_point;
            for action in output.actions {
                self.session.apply(action, self.canvas_rect);
            }
        }
        self.session.fit_if_pending(self.canvas_rect);
        let title = self.session.window_title();
        if title != self.title {
            self.window.set_title(&title);
            self.title = title;
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
        self.session.prepare_frame(&mut encoder, self.gpu.size());
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
            self.session.draw(&mut pass);
            self.egui_renderer.render(&mut pass, &paint_jobs, &screen);
        }

        // Canvas work is submitted even when no frame could be acquired.
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
