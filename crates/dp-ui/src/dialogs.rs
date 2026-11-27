use egui::{Id, Modal};

use crate::{Action, Frame, UiState};

pub(crate) fn show(ctx: &egui::Context, state: &mut UiState, frame: &Frame<'_>, actions: &mut Vec<Action>) {
    // One modal at a time, most urgent first.
    if let Some(error) = frame.error {
        Modal::new(Id::unique("error")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading("No se pudo completar la operación");
            ui.label(error);
            ui.add_space(8.0);
            if ui.button("Aceptar").clicked() {
                actions.push(Action::DismissError);
            }
        });
    } else if frame.confirm_unsaved {
        Modal::new(Id::unique("unsaved")).show(ctx, |ui| {
            ui.heading("Hay cambios sin guardar");
            ui.label("Si continúas sin guardar, se perderán.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Guardar").clicked() {
                    actions.push(Action::UnsavedSave);
                }
                if ui.button("Descartar").clicked() {
                    actions.push(Action::UnsavedDiscard);
                }
                if ui.button("Cancelar").clicked() {
                    actions.push(Action::UnsavedCancel);
                }
            });
        });
    } else if let Some(size) = &mut state.new_canvas {
        let max = frame.max_canvas_side;
        let mut close = false;
        Modal::new(Id::unique("new_canvas")).show(ctx, |ui| {
            ui.heading("Lienzo nuevo");
            egui::Grid::new("new_canvas_size").show(ui, |ui| {
                ui.label("Ancho");
                ui.add(egui::DragValue::new(&mut size[0]).range(1..=max).suffix(" px"));
                ui.end_row();
                ui.label("Alto");
                ui.add(egui::DragValue::new(&mut size[1]).range(1..=max).suffix(" px"));
                ui.end_row();
            });
            ui.weak(format!("Máximo en este equipo: {max} px por lado"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Crear").clicked() {
                    actions.push(Action::New { width: size[0], height: size[1] });
                    close = true;
                }
                if ui.button("Cancelar").clicked() {
                    close = true;
                }
            });
        });
        if close {
            state.new_canvas = None;
        }
    }
}
