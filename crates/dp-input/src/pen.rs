//! Platform-independent pieces of pen handling.

use crate::Phase;
use glam::Vec2;

/// Turns a stream of "is the tip touching?" flags into stroke phases.
///
/// Deriving the phase from contact transitions, rather than from the message
/// type, stays correct when several history points are delivered at once.
#[derive(Debug, Default, Clone, Copy)]
pub struct ContactTracker {
    in_contact: bool,
}

impl ContactTracker {
    pub fn update(&mut self, contact: bool) -> Phase {
        let phase = match (self.in_contact, contact) {
            (false, false) => Phase::Hover,
            (false, true) => Phase::Down,
            (true, true) => Phase::Move,
            (true, false) => Phase::Up,
        };
        self.in_contact = contact;
        phase
    }

    pub fn in_contact(&self) -> bool {
        self.in_contact
    }
}

/// An axis-aligned rectangle given by its edges.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Edges {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Maps a digitizer coordinate to screen pixels, keeping the sub-pixel part.
///
/// `device` is the digitizer's extent in its own units and `display` the
/// screen rectangle it is mapped to. Returns `None` for degenerate rectangles.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn digitizer_to_screen(point: Vec2, device: Edges, display: Edges) -> Option<Vec2> {
    let device_size = Vec2::new(device.right - device.left, device.bottom - device.top);
    let display_size = Vec2::new(display.right - display.left, display.bottom - display.top);
    if device_size.min_element() <= 0.0 || display_size.min_element() <= 0.0 {
        return None;
    }
    let normalized = (point - Vec2::new(device.left, device.top)) / device_size;
    Some(Vec2::new(display.left, display.top) + normalized * display_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_transitions() {
        let mut t = ContactTracker::default();
        let phases: Vec<Phase> =
            [false, true, true, false, false, true, false].into_iter().map(|c| t.update(c)).collect();
        use Phase::*;
        assert_eq!(phases, vec![Hover, Down, Move, Up, Hover, Down, Up]);
        assert!(!t.in_contact());
    }

    #[test]
    fn digitizer_mapping_keeps_subpixels() {
        let device = Edges { left: 0.0, top: 0.0, right: 30000.0, bottom: 20000.0 };
        let display = Edges { left: 1920.0, top: 0.0, right: 3840.0, bottom: 1080.0 };
        let p = digitizer_to_screen(Vec2::new(15004.0, 10000.0), device, display).unwrap();
        assert!((p.x - 2880.256).abs() < 1e-3);
        assert!((p.y - 540.0).abs() < 1e-3);
    }

    #[test]
    fn degenerate_rects_are_rejected() {
        let zero = Edges { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };
        let ok = Edges { left: 0.0, top: 0.0, right: 10.0, bottom: 10.0 };
        assert!(digitizer_to_screen(Vec2::ONE, zero, ok).is_none());
        assert!(digitizer_to_screen(Vec2::ONE, ok, zero).is_none());
    }
}
