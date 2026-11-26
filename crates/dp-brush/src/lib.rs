//! Brush engine: turns pointer positions and pressure into dabs.
//!
//! This crate is pure CPU logic with no GPU dependency, so strokes are
//! deterministic and testable. Phase 0 implements linear interpolation with
//! distance-based spacing; the stabilizer, curve interpolation and richer
//! dynamics come in phase 1.

use glam::Vec2;

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
    pub flow: f32,
    pub hardness: f32,
    pub pressure_affects_size: bool,
    pub pressure_affects_flow: bool,
}

impl Default for BrushParams {
    fn default() -> Self {
        Self {
            size: 12.0,
            min_size_ratio: 0.1,
            spacing: 0.1,
            flow: 1.0,
            hardness: 0.8,
            pressure_affects_size: true,
            pressure_affects_flow: false,
        }
    }
}

impl BrushParams {
    const MIN_RADIUS: f32 = 0.25;
    const MIN_STEP: f32 = 0.25;

    fn dab(&self, center: Vec2, pressure: f32) -> Dab {
        let pressure = pressure.clamp(0.0, 1.0);
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
}

/// Generates the dabs of a single stroke.
#[derive(Debug, Clone)]
pub struct StrokeBuilder {
    params: BrushParams,
    last_pos: Vec2,
    last_pressure: f32,
    /// Distance left to travel from `last_pos` before the next dab.
    until_next: f32,
}

impl StrokeBuilder {
    /// Starts a stroke, emitting its first dab.
    pub fn begin(params: BrushParams, pos: Vec2, pressure: f32, out: &mut Vec<Dab>) -> Self {
        let dab = params.dab(pos, pressure);
        out.push(dab);
        Self { params, last_pos: pos, last_pressure: pressure, until_next: params.step_for(dab.radius) }
    }

    /// Extends the stroke to `pos`, emitting evenly spaced dabs along the way.
    pub fn extend(&mut self, pos: Vec2, pressure: f32, out: &mut Vec<Dab>) {
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
}
