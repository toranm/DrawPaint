use dp_core::{BlendMode, LayerProps};
use dp_input::DeviceKind;
use egui::{Button, Slider};

use crate::{Action, Frame, UiState};

/// Widest brush offered by the size slider, in canvas pixels.
const MAX_BRUSH_SIZE: f32 = 500.0;
const SIDE_PANEL_WIDTH: f32 = 230.0;

pub(crate) fn menu_bar(ui: &mut egui::Ui, state: &mut UiState, frame: &Frame<'_>, actions: &mut Vec<Action>) {
    egui::Panel::top("menu_bar").show(ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("Archivo", |ui| {
                if ui.add(Button::new("Nuevo…").shortcut_text("Ctrl+N")).clicked() {
                    let document = frame.document;
                    state.new_canvas = Some([document.width(), document.height()]);
                }
                let mut item = |ui: &mut egui::Ui, label: &str, shortcut: &str, action: Action| {
                    if ui.add(Button::new(label).shortcut_text(shortcut)).clicked() {
                        actions.push(action);
                    }
                };
                item(ui, "Abrir…", "Ctrl+O", Action::Open);
                ui.separator();
                item(ui, "Guardar", "Ctrl+S", Action::Save);
                item(ui, "Guardar como…", "Ctrl+Mayús+S", Action::SaveAs);
                item(ui, "Exportar imagen…", "Ctrl+E", Action::Export);
            });
            ui.menu_button("Editar", |ui| {
                let undo = Button::new("Deshacer").shortcut_text("Ctrl+Z");
                if ui.add_enabled(frame.can_undo, undo).clicked() {
                    actions.push(Action::Undo);
                }
                let redo = Button::new("Rehacer").shortcut_text("Ctrl+Y");
                if ui.add_enabled(frame.can_redo, redo).clicked() {
                    actions.push(Action::Redo);
                }
            });
            ui.menu_button("Ver", |ui| {
                let mut item = |ui: &mut egui::Ui, label: &str, shortcut: &str, action: Action| {
                    if ui.add(Button::new(label).shortcut_text(shortcut)).clicked() {
                        actions.push(action);
                    }
                };
                item(ui, "Ajustar a la ventana", "Ctrl+0", Action::FitView);
                item(ui, "Tamaño real", "Ctrl+1", Action::ActualSize);
                item(ui, "Enderezar", "", Action::ResetRotation);
            });
            ui.separator();
            if ui.add_enabled(frame.can_undo, Button::new("Deshacer")).on_hover_text("Ctrl+Z").clicked() {
                actions.push(Action::Undo);
            }
            if ui.add_enabled(frame.can_redo, Button::new("Rehacer")).on_hover_text("Ctrl+Y").clicked() {
                actions.push(Action::Redo);
            }
            ui.separator();
            let document = frame.document;
            ui.label(format!(
                "{}{} · {} × {} px · {:.0} % · {:.0}°",
                frame.file_name.unwrap_or("Sin título"),
                if frame.modified { " •" } else { "" },
                document.width(),
                document.height(),
                frame.zoom * 100.0,
                frame.rotation_degrees
            ));
        });
    });
}

pub(crate) fn brush_panel(ui: &mut egui::Ui, state: &mut UiState, frame: &Frame<'_>) {
    egui::Panel::left("brush").resizable(false).default_size(SIDE_PANEL_WIDTH).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);
            ui.heading("Pinceles");
            ui.horizontal_wrapped(|ui| {
                for (index, preset) in dp_brush::presets().iter().enumerate() {
                    if ui.selectable_label(state.preset == index, preset.name).clicked() {
                        state.select_preset(index);
                    }
                }
            });

            ui.separator();
            ui.heading("Color");
            let [r, g, b] = state.color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
            let mut color = egui::Color32::from_rgb(r, g, b);
            let alpha = egui::color_picker::Alpha::Opaque;
            if egui::color_picker::color_picker_color32(ui, &mut color, alpha) {
                state.color = [color.r(), color.g(), color.b()].map(|c| f32::from(c) / 255.0);
            }

            ui.separator();
            ui.heading("Ajustes");
            let brush = &mut state.brush;
            ui.add(
                Slider::new(&mut brush.size, 1.0..=MAX_BRUSH_SIZE)
                    .logarithmic(true)
                    .suffix(" px")
                    .text("Tamaño"),
            );
            ui.add(percent_slider(&mut brush.opacity, "Opacidad"));
            ui.add(percent_slider(&mut brush.flow, "Flujo"));
            ui.add(percent_slider(&mut brush.hardness, "Dureza"));
            ui.add(percent_slider(&mut brush.min_size_ratio, "Tamaño mín."));
            ui.add(percent_slider(&mut brush.stabilizer, "Estabilizador"));
            ui.add(
                Slider::new(&mut brush.pressure_gamma, 0.25..=4.0).logarithmic(true).text("Curva de presión"),
            )
            .on_hover_text("Menos de 1: responde a un toque ligero. Más de 1: pide más fuerza.");
            ui.checkbox(&mut brush.pressure_affects_size, "La presión cambia el tamaño");
            ui.checkbox(&mut brush.pressure_affects_flow, "La presión cambia el flujo");
            ui.checkbox(&mut brush.erase, "Borrar");

            ui.separator();
            ui.collapsing("Puntero", |ui| {
                let pointer = frame.pointer;
                let device = match pointer.device {
                    Some(DeviceKind::Pen) => "Lápiz",
                    Some(DeviceKind::Eraser) => "Borrador",
                    Some(DeviceKind::Touch) => "Táctil",
                    Some(DeviceKind::Mouse) => "Ratón",
                    None => "—",
                };
                ui.label(format!("Dispositivo: {device}"));
                ui.label(format!("Presión: {:.3}", pointer.pressure));
                ui.label(format!(
                    "Inclinación: {:.0}°, {:.0}°",
                    pointer.tilt_degrees[0], pointer.tilt_degrees[1]
                ));
                ui.label(format!("Muestras/s: {}", pointer.samples_per_second));
                ui.label(if pointer.native_pen_backend {
                    "Lápiz nativo: activo"
                } else {
                    "Lápiz nativo: no disponible"
                });
            });
            ui.collapsing("Controles", |ui| {
                ui.label("Rueda: zoom");
                ui.label("Mayús + rueda: rotar");
                ui.label("Botón central o Espacio + arrastrar: mover");
                ui.label("Alt + clic: tomar color");
                ui.label("[ y ]: tamaño del pincel");
                ui.label("B: pincel · E: borrador");
            });
        });
    });
}

pub(crate) fn layer_panel(ui: &mut egui::Ui, frame: &Frame<'_>, actions: &mut Vec<Action>) {
    let document = frame.document;
    let active = document.active_layer();
    let active_index = document.index_of(active.id).unwrap_or(0);
    let count = document.layers().len();

    egui::Panel::right("layers").resizable(false).default_size(SIDE_PANEL_WIDTH).show(ui, |ui| {
        ui.add_space(4.0);
        ui.heading("Capas");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(document.can_add_layer(), Button::new("Nueva"))
                .on_hover_text("Nueva capa sobre la activa")
                .clicked()
            {
                actions.push(Action::AddLayer);
            }
            if ui.add_enabled(count > 1, Button::new("Borrar")).on_hover_text("Borrar capa").clicked() {
                actions.push(Action::DeleteLayer(active.id));
            }
            if ui
                .add_enabled(active_index + 1 < count, Button::new("Subir"))
                .on_hover_text("Subir capa")
                .clicked()
            {
                actions.push(Action::MoveLayer { id: active.id, up: true });
            }
            if ui.add_enabled(active_index > 0, Button::new("Bajar")).on_hover_text("Bajar capa").clicked() {
                actions.push(Action::MoveLayer { id: active.id, up: false });
            }
        });

        // Properties of the active layer. Each widget reports whether its
        // change is complete, so a slider drag becomes one undo step.
        let mut props = active.props.clone();
        let mut finished = false;
        egui::ComboBox::from_label("Fusión").selected_text(blend_name(props.blend)).show_ui(ui, |ui| {
            for mode in BlendMode::ALL {
                finished |= ui.selectable_value(&mut props.blend, mode, blend_name(mode)).changed();
            }
        });
        let opacity = ui.add(percent_slider(&mut props.opacity, "Opacidad"));
        finished |= opacity.drag_stopped() || opacity.lost_focus();
        let name = ui.add(egui::TextEdit::singleline(&mut props.name).hint_text("Nombre"));
        finished |= name.lost_focus();
        if props != active.props || finished {
            actions.push(Action::SetLayerProps { id: active.id, props, finished });
        }

        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            // The top layer is listed first.
            for layer in document.layers().iter().rev() {
                ui.horizontal(|ui| {
                    let mut visible = layer.props.visible;
                    if ui.checkbox(&mut visible, "").on_hover_text("Visible").changed() {
                        let props = LayerProps { visible, ..layer.props.clone() };
                        actions.push(Action::SetLayerProps { id: layer.id, props, finished: true });
                    }
                    let selected = layer.id == active.id;
                    if ui.selectable_label(selected, &layer.props.name).clicked() && !selected {
                        actions.push(Action::SelectLayer(layer.id));
                    }
                });
            }
        });
    });
}

fn blend_name(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "Normal",
        BlendMode::Multiply => "Multiplicar",
        BlendMode::Screen => "Trama",
        BlendMode::Overlay => "Superponer",
        BlendMode::Add => "Sumar",
    }
}

fn percent_slider<'a>(value: &'a mut f32, label: &'a str) -> Slider<'a> {
    Slider::new(value, 0.0..=1.0)
        .custom_formatter(|v, _| format!("{:.0} %", v * 100.0))
        .custom_parser(|s| s.trim().trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0))
        .text(label)
}
