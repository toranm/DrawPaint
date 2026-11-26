//! The egui user interface: panels around the canvas.
//!
//! This crate only describes widgets and reports what the user asked for;
//! it never touches the GPU or the document directly.
//!
//! UI strings are Spanish for now. Translations arrive with the i18n work
//! planned for phase 4.

use dp_brush::BrushParams;
use dp_input::DeviceKind;

/// Everything the panels can edit.
#[derive(Debug, Clone, PartialEq)]
pub struct UiState {
    pub brush: BrushParams,
    /// Straight sRGB-encoded brush colour.
    pub color: [f32; 3],
}

impl Default for UiState {
    fn default() -> Self {
        Self { brush: BrushParams::default(), color: [0.1, 0.1, 0.1] }
    }
}

/// Read-only view information shown in the top bar.
#[derive(Debug, Clone, Copy)]
pub struct ViewInfo {
    pub zoom: f32,
    pub rotation_degrees: f32,
    pub canvas_size: [u32; 2],
}

/// Diagnostics about the pointer, used to validate tablet support.
#[derive(Debug, Clone, Copy, Default)]
pub struct PointerInfo {
    pub native_pen_backend: bool,
    pub device: Option<DeviceKind>,
    pub pressure: f32,
    pub tilt_degrees: [f32; 2],
    pub samples_per_second: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    FitView,
    ActualSize,
    ResetRotation,
    ClearCanvas,
}

#[derive(Debug)]
pub struct UiOutput {
    /// The area left for the canvas, in egui points.
    pub canvas_rect: egui::Rect,
    pub actions: Vec<Action>,
}

/// Widest brush offered by the size slider, in canvas pixels.
const MAX_BRUSH_SIZE: f32 = 500.0;

pub fn show(ui: &mut egui::Ui, state: &mut UiState, view: ViewInfo, pointer: PointerInfo) -> UiOutput {
    let mut actions = Vec::new();

    egui::Panel::top("top_bar").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.strong("DrawPaint");
            ui.separator();
            if ui.button("Ajustar").on_hover_text("Ctrl+0").clicked() {
                actions.push(Action::FitView);
            }
            if ui.button("100 %").on_hover_text("Ctrl+1").clicked() {
                actions.push(Action::ActualSize);
            }
            if ui.button("Enderezar").clicked() {
                actions.push(Action::ResetRotation);
            }
            ui.separator();
            if ui.button("Limpiar lienzo").clicked() {
                actions.push(Action::ClearCanvas);
            }
            ui.separator();
            ui.label(format!(
                "{} × {} px · {:.0} % · {:.0}°",
                view.canvas_size[0],
                view.canvas_size[1],
                view.zoom * 100.0,
                view.rotation_degrees
            ));
        });
    });

    egui::Panel::left("tools").resizable(false).default_size(220.0).show(ui, |ui| {
        ui.add_space(4.0);
        ui.heading("Pincel");
        ui.horizontal(|ui| {
            ui.label("Color");
            ui.color_edit_button_rgb(&mut state.color);
        });
        let brush = &mut state.brush;
        ui.add(
            egui::Slider::new(&mut brush.size, 1.0..=MAX_BRUSH_SIZE)
                .logarithmic(true)
                .suffix(" px")
                .text("Tamaño"),
        );
        ui.add(percent_slider(&mut brush.flow, "Flujo"));
        ui.add(percent_slider(&mut brush.hardness, "Dureza"));
        ui.add(percent_slider(&mut brush.min_size_ratio, "Tamaño mín."));
        ui.checkbox(&mut brush.pressure_affects_size, "La presión cambia el tamaño");
        ui.checkbox(&mut brush.pressure_affects_flow, "La presión cambia el flujo");

        ui.separator();
        ui.heading("Puntero");
        let device = match pointer.device {
            Some(DeviceKind::Pen) => "Lápiz",
            Some(DeviceKind::Eraser) => "Borrador",
            Some(DeviceKind::Touch) => "Táctil",
            Some(DeviceKind::Mouse) => "Ratón",
            None => "—",
        };
        ui.label(format!("Dispositivo: {device}"));
        ui.label(format!("Presión: {:.3}", pointer.pressure));
        ui.label(format!("Inclinación: {:.0}°, {:.0}°", pointer.tilt_degrees[0], pointer.tilt_degrees[1]));
        ui.label(format!("Muestras/s: {}", pointer.samples_per_second));
        ui.label(if pointer.native_pen_backend {
            "Lápiz nativo: activo"
        } else {
            "Lápiz nativo: no disponible"
        });

        ui.separator();
        ui.heading("Controles");
        ui.label("Rueda: zoom");
        ui.label("Mayús + rueda: rotar");
        ui.label("Botón central o Espacio + arrastrar: mover");
    });

    UiOutput { canvas_rect: ui.available_rect_before_wrap(), actions }
}

fn percent_slider<'a>(value: &'a mut f32, label: &'a str) -> egui::Slider<'a> {
    egui::Slider::new(value, 0.0..=1.0)
        .custom_formatter(|v, _| format!("{:.0} %", v * 100.0))
        .custom_parser(|s| s.trim().trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0))
        .text(label)
}
