//! Mapping between canvas pixels and screen pixels.

use glam::{Affine2, Vec2};

/// How the canvas is placed on screen: `screen = offset + zoom * R(rotation) * canvas`.
///
/// All screen coordinates are physical pixels with the origin at the top-left
/// of the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewTransform {
    /// Screen position of the canvas origin (its top-left corner).
    pub offset: Vec2,
    /// Screen pixels per canvas pixel.
    pub zoom: f32,
    /// Clockwise rotation in radians (screen Y points down).
    pub rotation: f32,
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self { offset: Vec2::ZERO, zoom: 1.0, rotation: 0.0 }
    }
}

impl ViewTransform {
    pub const MIN_ZOOM: f32 = 0.01;
    pub const MAX_ZOOM: f32 = 64.0;

    pub fn canvas_to_screen(&self) -> Affine2 {
        Affine2::from_scale_angle_translation(Vec2::splat(self.zoom), self.rotation, self.offset)
    }

    pub fn screen_to_canvas(&self) -> Affine2 {
        self.canvas_to_screen().inverse()
    }

    pub fn to_screen(&self, canvas: Vec2) -> Vec2 {
        self.canvas_to_screen().transform_point2(canvas)
    }

    pub fn to_canvas(&self, screen: Vec2) -> Vec2 {
        self.screen_to_canvas().transform_point2(screen)
    }

    pub fn pan(&mut self, screen_delta: Vec2) {
        self.offset += screen_delta;
    }

    /// Multiplies the zoom by `factor`, keeping the canvas point under
    /// `screen_anchor` fixed on screen.
    pub fn zoom_about(&mut self, screen_anchor: Vec2, factor: f32) {
        let new_zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        let factor = new_zoom / self.zoom;
        self.offset = screen_anchor + (self.offset - screen_anchor) * factor;
        self.zoom = new_zoom;
    }

    /// Rotates the view by `angle` radians around `screen_anchor`.
    pub fn rotate_about(&mut self, screen_anchor: Vec2, angle: f32) {
        self.offset = screen_anchor + Vec2::from_angle(angle).rotate(self.offset - screen_anchor);
        self.rotation = (self.rotation + angle).rem_euclid(std::f32::consts::TAU);
    }

    /// Centers the canvas in the viewport rectangle, unrotated, scaled to fit
    /// with a small margin.
    pub fn fit(canvas_size: Vec2, viewport_min: Vec2, viewport_size: Vec2) -> Self {
        const MARGIN: f32 = 0.95;
        let scale = (viewport_size / canvas_size).min_element() * MARGIN;
        let zoom =
            if scale.is_finite() && scale > 0.0 { scale.clamp(Self::MIN_ZOOM, Self::MAX_ZOOM) } else { 1.0 };
        let offset = viewport_min + (viewport_size - canvas_size * zoom) * 0.5;
        Self { offset, zoom, rotation: 0.0 }
    }

    /// Sets the zoom to exactly `zoom`, keeping `screen_anchor` fixed.
    pub fn set_zoom_about(&mut self, screen_anchor: Vec2, zoom: f32) {
        self.zoom_about(screen_anchor, zoom / self.zoom);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec2;

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).abs().max_element() < 1e-2
    }

    #[test]
    fn round_trip() {
        let v = ViewTransform { offset: vec2(120.0, -40.0), zoom: 2.5, rotation: 0.7 };
        let p = vec2(321.0, 77.5);
        assert!(close(v.to_canvas(v.to_screen(p)), p));
    }

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let mut v = ViewTransform { offset: vec2(10.0, 20.0), zoom: 1.5, rotation: 0.3 };
        let anchor = vec2(400.0, 300.0);
        let before = v.to_canvas(anchor);
        v.zoom_about(anchor, 3.0);
        assert!((v.zoom - 4.5).abs() < 1e-5);
        assert!(close(v.to_canvas(anchor), before));
    }

    #[test]
    fn zoom_is_clamped_and_still_anchored() {
        let mut v = ViewTransform::default();
        let anchor = vec2(50.0, 50.0);
        let before = v.to_canvas(anchor);
        v.zoom_about(anchor, 1e6);
        assert_eq!(v.zoom, ViewTransform::MAX_ZOOM);
        assert!(close(v.to_canvas(anchor), before));
    }

    #[test]
    fn rotation_keeps_anchor_fixed() {
        let mut v = ViewTransform { offset: vec2(10.0, 20.0), zoom: 0.8, rotation: 0.0 };
        let anchor = vec2(200.0, 150.0);
        let before = v.to_canvas(anchor);
        v.rotate_about(anchor, 1.1);
        assert!(close(v.to_canvas(anchor), before));
        assert!((v.rotation - 1.1).abs() < 1e-5);
    }

    #[test]
    fn fit_centers_canvas() {
        let canvas = vec2(2000.0, 1000.0);
        let v = ViewTransform::fit(canvas, vec2(100.0, 50.0), vec2(800.0, 600.0));
        let center = v.to_screen(canvas * 0.5);
        assert!(close(center, vec2(500.0, 350.0)));
        assert!((v.zoom - 0.38).abs() < 1e-4);
    }
}
