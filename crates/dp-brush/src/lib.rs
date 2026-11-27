//! Brush engine: turns pointer positions and pressure into dabs.
//!
//! This crate is pure CPU logic with no GPU dependency, so strokes are
//! deterministic and testable.

use glam::Vec2;

mod presets;

pub use presets::{presets, BrushPreset};

/// One stamp of the brush tip, in canvas pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dab {
    pub center: Vec2,
    pub radius: f32,
    /// Opacity of this single dab, 0..=1.
    pub flow: f32,
    /// 0 = fully soft edge, 1 = hard edge.
    pub hardness: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushParams {
    /// Diameter in canvas pixels at full pressure.
    pub size: f32,
    /// Diameter at zero pressure, as a fraction of `size`.
    pub min_size_ratio: f32,
    /// Distance between dabs as a fraction of the current diameter.
    pub spacing: f32,
    /// Opacity of each dab. Overlapping dabs build up.
    pub flow: f32,
    /// Ceiling for the opacity of the whole stroke, however much it overlaps
    /// itself. Applied when the stroke is composited, not per dab.
    pub opacity: f32,
    pub hardness: f32,
    pub pressure_affects_size: bool,
    pub pressure_affects_flow: bool,
    /// Exponent applied to pressure: below 1 the brush responds to a light
    /// touch, above 1 it needs a firmer hand.
    pub pressure_gamma: f32,
    /// 0 = follow the pointer exactly, 1 = heaviest smoothing.
    pub stabilizer: f32,
    /// Removes paint instead of adding it.
    pub erase: bool,
}

impl Default for BrushParams {
    fn default() -> Self {
        Self {
            size: 12.0,
            min_size_ratio: 0.1,
            spacing: 0.1,
            flow: 1.0,
            opacity: 1.0,
            hardness: 0.8,
            pressure_affects_size: true,
            pressure_affects_flow: false,
            pressure_gamma: 1.0,
            stabilizer: 0.0,
            erase: false,
        }
    }
}

impl BrushParams {
    const MIN_RADIUS: f32 = 0.25;
    const MIN_STEP: f32 = 0.25;
    /// Share of the remaining distance covered per sample at full stabilizer.
    const MIN_FOLLOW: f32 = 0.04;

    fn dab(&self, center: Vec2, pressure: f32) -> Dab {
        let pressure = pressure.clamp(0.0, 1.0).powf(self.pressure_gamma.max(0.01));
        let size_scale = if self.pressure_affects_size {
            self.min_size_ratio + (1.0 - self.min_size_ratio) * pressure
        } else {
            1.0
        };
        let flow_scale = if self.pressure_affects_flow { pressure } else { 1.0 };
        Dab {
            center,
            radius: (self.size * 0.5 * size_scale).max(Self::MIN_RADIUS),
            flow: (self.flow * flow_scale).clamp(0.0, 1.0),
            hardness: self.hardness.clamp(0.0, 1.0),
        }
    }

    fn step_for(&self, radius: f32) -> f32 {
        (radius * 2.0 * self.spacing).max(Self::MIN_STEP)
    }

    /// How far the smoothed point moves towards the pointer per sample.
    fn follow(&self) -> f32 {
        1.0 - (1.0 - Self::MIN_FOLLOW) * self.stabilizer.clamp(0.0, 1.0)
    }
}

/// Generates the dabs of a single stroke.
///
/// With the stabilizer on, the drawn point trails the pointer along a
/// smoothed path; [`StrokeBuilder::finish`] closes the remaining gap.
#[derive(Debug, Clone)]
pub struct StrokeBuilder {
    params: BrushParams,
    last_pos: Vec2,
    last_pressure: f32,
    /// Distance left to travel from `last_pos` before the next dab.
    until_next: f32,
    /// Latest raw pointer position and pressure.
    target: (Vec2, f32),
}

impl StrokeBuilder {
    /// Starts a stroke, emitting its first dab.
    pub fn begin(params: BrushParams, pos: Vec2, pressure: f32, out: &mut Vec<Dab>) -> Self {
        let dab = params.dab(pos, pressure);
        out.push(dab);
        Self {
            params,
            last_pos: pos,
            last_pressure: pressure,
            until_next: params.step_for(dab.radius),
            target: (pos, pressure),
        }
    }

    pub fn params(&self) -> &BrushParams {
        &self.params
    }

    /// Feeds a new pointer sample, emitting evenly spaced dabs along the
    /// (possibly smoothed) path.
    pub fn extend(&mut self, pos: Vec2, pressure: f32, out: &mut Vec<Dab>) {
        self.target = (pos, pressure);
        let follow = self.params.follow();
        let smoothed_pos = self.last_pos + (pos - self.last_pos) * follow;
        let smoothed_pressure = self.last_pressure + (pressure - self.last_pressure) * follow;
        self.line_to(smoothed_pos, smoothed_pressure, out);
    }

    /// Ends the stroke where the pointer actually lifted.
    pub fn finish(mut self, out: &mut Vec<Dab>) {
        let (pos, pressure) = self.target;
        self.line_to(pos, pressure, out);
    }

    fn line_to(&mut self, pos: Vec2, pressure: f32, out: &mut Vec<Dab>) {
        let delta = pos - self.last_pos;
        let length = delta.length();
        if !length.is_finite() {
            return;
        }
        let mut travelled = 0.0;
        while length - travelled >= self.until_next {
            travelled += self.until_next;
            let t = travelled / length;
            let dab = self
                .params
                .dab(self.last_pos + delta * t, self.last_pressure + (pressure - self.last_pressure) * t);
            out.push(dab);
            self.until_next = self.params.step_for(dab.radius);
        }
        self.until_next -= length - travelled;
        self.last_pos = pos;
        self.last_pressure = pressure;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec2;

    fn fixed(size: f32, spacing: f32) -> BrushParams {
        BrushParams { size, spacing, pressure_affects_size: false, ..Default::default() }
    }

    #[test]
    fn begin_emits_one_dab() {
        let mut out = Vec::new();
        StrokeBuilder::begin(fixed(10.0, 0.5), vec2(3.0, 4.0), 1.0, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].center, vec2(3.0, 4.0));
        assert_eq!(out[0].radius, 5.0);
    }

    #[test]
    fn dabs_are_evenly_spaced() {
        let mut out = Vec::new();
        let mut s = StrokeBuilder::begin(fixed(10.0, 0.5), Vec2::ZERO, 1.0, &mut out);
        s.extend(vec2(22.0, 0.0), 1.0, &mut out);
        let xs: Vec<f32> = out.iter().map(|d| d.center.x).collect();
        assert_eq!(xs, vec![0.0, 5.0, 10.0, 15.0, 20.0]);
    }

    #[test]
    fn spacing_carries_across_segments() {
        // Many tiny moves must produce the same dabs as one long move.
        let mut one = Vec::new();
        let mut s = StrokeBuilder::begin(fixed(10.0, 0.5), Vec2::ZERO, 1.0, &mut one);
        // End between two dab positions so rounding cannot change the count.
        s.extend(vec2(42.0, 0.0), 1.0, &mut one);

        let mut many = Vec::new();
        let mut s = StrokeBuilder::begin(fixed(10.0, 0.5), Vec2::ZERO, 1.0, &mut many);
        for i in 1..=420 {
            s.extend(vec2(i as f32 * 0.1, 0.0), 1.0, &mut many);
        }
        assert_eq!(one.len(), many.len());
        for (a, b) in one.iter().zip(&many) {
            assert!((a.center - b.center).length() < 1e-2);
        }
    }

    #[test]
    fn stationary_pointer_adds_nothing() {
        let mut out = Vec::new();
        let mut s = StrokeBuilder::begin(fixed(10.0, 0.5), vec2(1.0, 1.0), 1.0, &mut out);
        s.extend(vec2(1.0, 1.0), 1.0, &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn pressure_scales_radius() {
        let params = BrushParams { size: 20.0, min_size_ratio: 0.0, ..Default::default() };
        let mut out = Vec::new();
        let mut s = StrokeBuilder::begin(params, Vec2::ZERO, 0.0, &mut out);
        s.extend(vec2(100.0, 0.0), 1.0, &mut out);
        assert!(out.len() > 10);
        assert!(out.windows(2).all(|w| w[1].radius >= w[0].radius));
        assert!(out.last().unwrap().radius > 9.0);
    }

    #[test]
    fn tiny_brush_cannot_explode_dab_count() {
        let params = BrushParams { size: 0.0, spacing: 0.0, ..Default::default() };
        let mut out = Vec::new();
        let mut s = StrokeBuilder::begin(params, Vec2::ZERO, 0.0, &mut out);
        s.extend(vec2(100.0, 0.0), 0.0, &mut out);
        assert!(out.len() <= 401);
    }

    #[test]
    fn pressure_gamma_reshapes_response() {
        let base = BrushParams { size: 100.0, min_size_ratio: 0.0, ..Default::default() };
        let radius = |gamma: f32| BrushParams { pressure_gamma: gamma, ..base }.dab(Vec2::ZERO, 0.25).radius;
        assert!((radius(1.0) - 12.5).abs() < 1e-3);
        assert!((radius(0.5) - 25.0).abs() < 1e-3);
        assert!((radius(2.0) - 3.125).abs() < 1e-3);
    }

    #[test]
    fn stabilizer_smooths_jitter() {
        // A horizontal line with alternating vertical noise.
        let run = |stabilizer: f32| {
            let params = BrushParams { stabilizer, ..fixed(4.0, 0.25) };
            let mut out = Vec::new();
            let mut s = StrokeBuilder::begin(params, Vec2::ZERO, 1.0, &mut out);
            for i in 1..=200 {
                let noise = if i % 2 == 0 { 3.0 } else { -3.0 };
                s.extend(vec2(i as f32 * 2.0, noise), 1.0, &mut out);
            }
            out.iter().map(|d| d.center.y.abs()).fold(0.0, f32::max)
        };
        assert!(run(0.0) > 2.9);
        assert!(run(0.8) < 1.0);
    }

    #[test]
    fn finish_reaches_the_pointer() {
        let params = BrushParams { stabilizer: 1.0, ..fixed(4.0, 0.25) };
        let mut out = Vec::new();
        let mut s = StrokeBuilder::begin(params, Vec2::ZERO, 1.0, &mut out);
        s.extend(vec2(100.0, 0.0), 1.0, &mut out);
        // Heavily stabilized: the stroke lags far behind the pointer...
        assert!(out.last().unwrap().center.x < 10.0);
        s.finish(&mut out);
        // ...until the pen lifts.
        assert!(out.last().unwrap().center.x > 99.0);
    }

    #[test]
    fn presets_are_sane() {
        let all = presets();
        assert_eq!(all.len(), 5);
        assert_eq!(all.iter().filter(|p| p.params.erase).count(), 1);
        for preset in all {
            let p = preset.params;
            assert!(p.size > 0.0 && p.spacing > 0.0, "{}", preset.name);
            assert!((0.0..=1.0).contains(&p.flow) && (0.0..=1.0).contains(&p.opacity), "{}", preset.name);
        }
    }
}
