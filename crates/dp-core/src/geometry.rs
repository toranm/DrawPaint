use glam::Vec2;

/// A rectangle of whole pixels. Empty when `width` or `height` is zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const EMPTY: Self = Self { x: 0, y: 0, width: 0, height: 0 };

    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    pub fn from_size(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Number of bytes of RGBA8 pixels this rectangle holds.
    pub fn byte_len(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }

    /// Smallest pixel rectangle covering the circle, clipped to `bounds`.
    pub fn around_circle(center: Vec2, radius: f32, bounds: Rect) -> Self {
        let clamp = |v: f32, max: u32| v.clamp(0.0, max as f32) as u32;
        let right = bounds.x + bounds.width;
        let bottom = bounds.y + bounds.height;
        let x0 = clamp((center.x - radius).floor(), right).max(bounds.x);
        let y0 = clamp((center.y - radius).floor(), bottom).max(bounds.y);
        let x1 = clamp((center.x + radius).ceil(), right);
        let y1 = clamp((center.y + radius).ceil(), bottom);
        Self::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
    }

    /// Smallest rectangle containing both. Empty rectangles are ignored.
    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.width).max(other.x + other.width);
        let y1 = (self.y + self.height).max(other.y + other.height);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec2;

    #[test]
    fn union_ignores_empty() {
        let a = Rect::new(10, 10, 5, 5);
        assert_eq!(a.union(Rect::EMPTY), a);
        assert_eq!(Rect::EMPTY.union(a), a);
        assert_eq!(a.union(Rect::new(0, 12, 4, 20)), Rect::new(0, 10, 15, 22));
    }

    #[test]
    fn circle_bounds_are_clipped() {
        let bounds = Rect::from_size(100, 100);
        assert_eq!(Rect::around_circle(vec2(50.5, 50.5), 2.0, bounds), Rect::new(48, 48, 5, 5));
        assert_eq!(Rect::around_circle(vec2(-1.0, 99.0), 3.0, bounds), Rect::new(0, 96, 2, 4));
        assert!(Rect::around_circle(vec2(-50.0, -50.0), 3.0, bounds).is_empty());
        assert!(Rect::around_circle(vec2(500.0, 50.0), 3.0, bounds).is_empty());
    }
}
