//! Document model and view math. No GPU, window or UI dependencies.

pub mod view;

pub use view::ViewTransform;

/// A painting document. Phase 0 only tracks the canvas size; layers, selection
/// and the command history arrive in phase 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Document {
    pub width: u32,
    pub height: u32,
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        assert!(width > 0 && height > 0, "canvas must not be empty");
        Self { width, height }
    }

    pub fn size(&self) -> glam::Vec2 {
        glam::vec2(self.width as f32, self.height as f32)
    }
}
