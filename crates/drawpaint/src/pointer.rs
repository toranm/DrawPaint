//! Merges the native pen backend and winit's pointer events into one stream
//! of [`PointerSample`]s.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use dp_input::{DeviceKind, NativePen, Phase, PointerSample};
use glam::Vec2;
use winit::event::{ElementState, MouseButton, Touch, TouchPhase};

/// After a pen sample, mouse events are ignored for this long: the system may
/// synthesize mouse input from the pen, which would draw every stroke twice.
const PEN_MOUSE_SUPPRESSION: Duration = Duration::from_millis(500);
const RATE_WINDOW: Duration = Duration::from_secs(1);

pub struct PointerInput {
    native: Option<NativePen>,
    start: Instant,
    cursor: Vec2,
    mouse_down: bool,
    active_touch: Option<u64>,
    last_pen: Option<Instant>,
    /// Arrival times of recent samples, for the samples-per-second readout.
    arrivals: VecDeque<Instant>,
    last_sample: Option<PointerSample>,
}

impl PointerInput {
    pub fn new(native: Option<NativePen>) -> Self {
        Self {
            native,
            start: Instant::now(),
            cursor: Vec2::ZERO,
            mouse_down: false,
            active_touch: None,
            last_pen: None,
            arrivals: VecDeque::new(),
            last_sample: None,
        }
    }

    pub fn has_native_pen(&self) -> bool {
        self.native.is_some()
    }

    pub fn cursor(&self) -> Vec2 {
        self.cursor
    }

    pub fn last_sample(&self) -> Option<PointerSample> {
        self.last_sample
    }

    pub fn samples_per_second(&mut self) -> u32 {
        let now = Instant::now();
        while self.arrivals.front().is_some_and(|t| now.duration_since(*t) > RATE_WINDOW) {
            self.arrivals.pop_front();
        }
        self.arrivals.len() as u32
    }

    /// Whether samples arrived recently enough that the readout is still
    /// counting down and the UI should keep refreshing.
    pub fn rate_is_settling(&self) -> bool {
        !self.arrivals.is_empty()
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn pen_recently_seen(&self) -> bool {
        self.last_pen.is_some_and(|t| t.elapsed() < PEN_MOUSE_SUPPRESSION)
    }

    fn record(&mut self, sample: PointerSample, out: &mut Vec<PointerSample>) {
        self.cursor = sample.pos;
        self.arrivals.push_back(Instant::now());
        self.last_sample = Some(sample);
        out.push(sample);
    }

    /// Collects samples from the native pen backend, if any.
    pub fn poll_native(&mut self, out: &mut Vec<PointerSample>) {
        let Some(native) = &self.native else { return };
        let mut samples = Vec::new();
        native.drain(&mut samples);
        for sample in samples {
            self.last_pen = Some(Instant::now());
            self.record(sample, out);
        }
    }

    pub fn cursor_moved(&mut self, pos: Vec2, out: &mut Vec<PointerSample>) {
        if self.pen_recently_seen() {
            return;
        }
        let phase = if self.mouse_down { Phase::Move } else { Phase::Hover };
        let sample = self.mouse_sample(pos, phase);
        self.record(sample, out);
    }

    pub fn mouse_button(&mut self, button: MouseButton, state: ElementState, out: &mut Vec<PointerSample>) {
        if button != MouseButton::Left || self.pen_recently_seen() {
            return;
        }
        let pressed = state == ElementState::Pressed;
        if pressed == self.mouse_down {
            return;
        }
        self.mouse_down = pressed;
        let sample = self.mouse_sample(self.cursor, if pressed { Phase::Down } else { Phase::Up });
        self.record(sample, out);
    }

    fn mouse_sample(&self, pos: Vec2, phase: Phase) -> PointerSample {
        PointerSample {
            device: DeviceKind::Mouse,
            phase,
            pos,
            pressure: if phase == Phase::Hover || phase == Phase::Up { 0.0 } else { 1.0 },
            tilt: Vec2::ZERO,
            twist: 0.0,
            time: self.now(),
        }
    }

    /// Handles winit touches, which is also how winit reports pens.
    pub fn touch(&mut self, touch: &Touch, out: &mut Vec<PointerSample>) {
        if self.native.is_some() {
            // The native backend reports the pen with more detail. Finger
            // painting and gestures are phase 2.
            return;
        }
        let phase = match touch.phase {
            TouchPhase::Started if self.active_touch.is_none() => {
                self.active_touch = Some(touch.id);
                Phase::Down
            }
            TouchPhase::Moved if self.active_touch == Some(touch.id) => Phase::Move,
            TouchPhase::Ended | TouchPhase::Cancelled if self.active_touch == Some(touch.id) => {
                self.active_touch = None;
                Phase::Up
            }
            _ => return, // A second finger.
        };
        let pressure = match phase {
            Phase::Up => 0.0,
            _ => touch.force.map_or(1.0, |f| f.normalized() as f32),
        };
        let sample = PointerSample {
            device: DeviceKind::Touch,
            phase,
            pos: Vec2::new(touch.location.x as f32, touch.location.y as f32),
            pressure,
            tilt: Vec2::ZERO,
            twist: 0.0,
            time: self.now(),
        };
        self.record(sample, out);
    }
}
