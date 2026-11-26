//! Unified pointer input.
//!
//! The windowing library reports pens as generic touches, without tilt and
//! without the intermediate points a tablet produces between two frames.
//! [`NativePen`] hooks the platform API directly to recover them. Where no
//! native backend exists yet, [`NativePen::install`] returns `None` and the
//! application falls back to the windowing library's events.

use glam::Vec2;

mod pen;
#[cfg(windows)]
mod windows;

pub use pen::ContactTracker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Mouse,
    Touch,
    Pen,
    /// The eraser end of a pen, or a pen with its eraser button held.
    Eraser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Moving in range of the surface without touching it.
    Hover,
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerSample {
    pub device: DeviceKind,
    pub phase: Phase,
    /// Position in physical pixels relative to the window's client area,
    /// with sub-pixel precision when the device provides it.
    pub pos: Vec2,
    /// 0..=1. Devices without pressure report 1 while in contact.
    pub pressure: f32,
    /// Tilt from vertical along X and Y, in degrees (-90..=90).
    pub tilt: Vec2,
    /// Rotation around the pen's own axis, in degrees (0..360).
    pub twist: f32,
    /// Device timestamp in seconds from an arbitrary origin.
    pub time: f64,
}

/// A native pen hook attached to one window. Dropping it detaches the hook.
pub struct NativePen {
    #[cfg(windows)]
    inner: windows::Hook,
}

impl NativePen {
    /// Attaches to `window`. Must be called on the thread that owns the window.
    ///
    /// Returns `None` when the platform has no native backend or the hook
    /// could not be installed.
    #[cfg_attr(not(windows), allow(unused_variables))]
    pub fn install(window: &impl raw_window_handle::HasWindowHandle) -> Option<Self> {
        #[cfg(windows)]
        {
            windows::Hook::install(window).map(|inner| Self { inner })
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// Moves every sample received since the last call into `out`, oldest first.
    #[cfg_attr(not(windows), allow(unused_variables, clippy::ptr_arg))]
    pub fn drain(&self, out: &mut Vec<PointerSample>) {
        #[cfg(windows)]
        self.inner.drain(out);
    }
}
