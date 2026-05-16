//! Per-sector-leg telemetry aggregates (throttle duty, slip extrema).

/// Accumulator for one timed subsection (from anchor to next sector cross).
#[derive(Debug, Clone, Default)]
pub struct SectorLegStatsAccumulator {
    sample_count: u64,
    throttle_open_samples: u64,
    max_slip_angle: f32,
    max_slip_ratio: f32,
    min_slip_ratio: f32,
    has_slip: bool,
}

/// Finalized stats stored with a sector split row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectorLegStatsSnapshot {
    /// Share of physics samples with gas > 0.9 (0–100).
    pub throttle_open_pct: f64,
    pub max_slip_angle: f32,
    pub max_slip_ratio: f32,
    pub min_slip_ratio: f32,
}

impl SectorLegStatsAccumulator {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Record one physics sample for the active leg.
    pub fn observe_sample(&mut self, gas: f32, slip_ratio: [f32; 4], slip_angle: [f32; 4]) {
        self.sample_count += 1;
        if gas > 0.9 {
            self.throttle_open_samples += 1;
        }

        let sa = slip_angle
            .iter()
            .map(|v| v.abs())
            .fold(0.0_f32, |a, b| a.max(b));
        let sr_max = slip_ratio.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let sr_min = slip_ratio.iter().copied().fold(f32::INFINITY, f32::min);

        if !self.has_slip {
            self.max_slip_angle = sa;
            self.max_slip_ratio = sr_max;
            self.min_slip_ratio = sr_min;
            self.has_slip = true;
        } else {
            self.max_slip_angle = self.max_slip_angle.max(sa);
            self.max_slip_ratio = self.max_slip_ratio.max(sr_max);
            self.min_slip_ratio = self.min_slip_ratio.min(sr_min);
        }
    }

    pub fn finalize(&self) -> Option<SectorLegStatsSnapshot> {
        if self.sample_count == 0 {
            return None;
        }
        let throttle_open_pct =
            self.throttle_open_samples as f64 / self.sample_count as f64 * 100.0;
        Some(SectorLegStatsSnapshot {
            throttle_open_pct,
            max_slip_angle: self.max_slip_angle,
            max_slip_ratio: self.max_slip_ratio,
            min_slip_ratio: self.min_slip_ratio,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_pct_and_slip_extrema() {
        let mut acc = SectorLegStatsAccumulator::default();
        acc.observe_sample(0.95, [0.1, 0.2, 0.0, 0.0], [1.0, 2.0, 0.5, 0.0]);
        acc.observe_sample(0.5, [0.3, -0.1, 0.0, 0.0], [-3.0, 1.0, 0.0, 0.0]);
        acc.observe_sample(1.0, [0.05, 0.05, 0.05, 0.05], [0.0, 0.0, 0.0, 0.0]);
        let s = acc.finalize().unwrap();
        assert!((s.throttle_open_pct - 200.0 / 3.0).abs() < 0.1); // 2/3 samples > 0.9
        assert!((s.max_slip_angle - 3.0).abs() < 1e-6);
        assert!((s.max_slip_ratio - 0.3).abs() < 1e-6);
        assert!((s.min_slip_ratio - (-0.1)).abs() < 1e-6);
    }
}
