//! Document model, edit history and view math. No GPU, window or UI
//! dependencies.

mod document;
mod geometry;
pub mod history;
pub mod view;

pub use document::{BlendMode, Document, DocumentError, Layer, LayerId, LayerProps};
pub use geometry::Rect;
pub use history::{Edit, History, PixelStore, Snapshot};
pub use view::ViewTransform;
