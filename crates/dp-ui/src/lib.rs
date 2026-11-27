//! The egui user interface: menus and panels around the canvas.
//!
//! This crate only describes widgets and reports what the user asked for as
//! [`Action`]s; it never touches the GPU or changes the document itself.
//!
//! UI strings are Spanish for now. Translations arrive with the i18n work
//! planned for phase 4.

mod dialogs;
mod panels;

use dp_brush::BrushParams;
use dp_core::{Document, LayerId, LayerProps};
use dp_input::DeviceKind;

/// Everything the panels own and can edit directly.
#[derive(Debug, Clone, PartialEq)]
pub struct UiState {
    pub brush: BrushParams,
    /// Index into [`dp_brush::presets`] of the preset the brush started from.
    pub preset: usize,
    /// Straight sRGB-encoded brush colour.
    pub color: [f32; 3],
    /// Size fields of the "new canvas" dialog while it is open.
    pub new_canvas: Option<[u32; 2]>,
}

impl Default for UiState {
    fn default() -> Self {
        Self { brush: dp_brush::presets()[0].params, preset: 0, color: [0.1, 0.1, 0.1], new_canvas: None }
    }
}

impl UiState {
    pub fn select_preset(&mut self, index: usize) {
        if let Some(preset) = dp_brush::presets().get(index) {
            self.preset = index;
            self.brush = preset.params;
        }
    }
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

/// Read-only application state the UI displays this frame.
pub struct Frame<'a> {
    pub document: &'a Document,
    /// File name of the document, if it has been saved.
    pub file_name: Option<&'a str>,
    pub modified: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub zoom: f32,
    pub rotation_degrees: f32,
    /// Largest canvas side the graphics device supports.
    pub max_canvas_side: u32,
    pub pointer: PointerInfo,
    /// Ask whether to save the unsaved changes before they are lost.
    pub confirm_unsaved: bool,
    /// An error to show until dismissed.
    pub error: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    New {
        width: u32,
        height: u32,
    },
    Open,
    Save,
    SaveAs,
    Export,
    Undo,
    Redo,
    FitView,
    ActualSize,
    ResetRotation,
    AddLayer,
    DeleteLayer(LayerId),
    /// Moves a layer one step up (towards the top) or down.
    MoveLayer {
        id: LayerId,
        up: bool,
    },
    SelectLayer(LayerId),
    /// `finished` is false while a slider is still being dragged.
    SetLayerProps {
        id: LayerId,
        props: LayerProps,
        finished: bool,
    },
    /// Answers to the unsaved-changes question.
    UnsavedSave,
    UnsavedDiscard,
    UnsavedCancel,
    DismissError,
}

#[derive(Debug)]
pub struct UiOutput {
    /// The area left for the canvas, in egui points.
    pub canvas_rect: egui::Rect,
    pub actions: Vec<Action>,
}

pub fn show(ui: &mut egui::Ui, state: &mut UiState, frame: &Frame<'_>) -> UiOutput {
    let mut actions = Vec::new();
    panels::menu_bar(ui, state, frame, &mut actions);
    panels::brush_panel(ui, state, frame);
    panels::layer_panel(ui, frame, &mut actions);
    dialogs::show(ui.ctx(), state, frame, &mut actions);
    UiOutput { canvas_rect: ui.available_rect_before_wrap(), actions }
}
