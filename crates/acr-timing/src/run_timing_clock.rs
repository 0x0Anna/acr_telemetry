//! Unified run timing: simulation time from `packet_id`, wall time for stall detection only.
//!
//! ACC live telemetry does not provide a usable in-game clock (`graphics.clock` is null).
//! All leg durations use Δpacket_id / physics_hz relative to anchors; optional stall
//! correction subtracts wall-time excess when `apply_leg_excess_correction` is enabled.

use std::time::Instant;

use crate::timing_frame_quality::TimingFrameMonitor;

#[derive(Debug, Clone, Copy)]
pub struct TimingAnchor {
    pub packet_id: i32,
    pub at: Instant,
    /// `graphics.distance_traveled` (not zero-based; use deltas between anchors).
    pub distance_traveled_m: f64,
    /// external timing provider rally time at anchor (when `[game_clock]` sync is enabled).
    pub game_race_sec: Option<f64>,
}

impl TimingAnchor {
    pub fn new(
        packet_id: i32,
        at: Instant,
        distance_traveled_m: f64,
        game_race_sec: Option<f64>,
    ) -> Self {
        Self {
            packet_id,
            at,
            distance_traveled_m,
            game_race_sec: game_race_sec.filter(|t| t.is_finite() && *t >= 0.0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunTimingClock {
    physics_hz: f64,
    run_origin: Option<TimingAnchor>,
    leg_anchor: Option<TimingAnchor>,
}

impl RunTimingClock {
    pub fn new(physics_hz: f64) -> Self {
        Self {
            physics_hz: physics_hz.max(1.0),
            run_origin: None,
            leg_anchor: None,
        }
    }

    pub fn reset(&mut self) {
        self.run_origin = None;
        self.leg_anchor = None;
    }

    pub fn run_origin(&self) -> Option<TimingAnchor> {
        self.run_origin
    }

    pub fn leg_anchor(&self) -> Option<TimingAnchor> {
        self.leg_anchor
    }

    /// Arm run + first leg at timing start or SHP/cumulative anchor.
    pub fn arm_run(&mut self, anchor: TimingAnchor) {
        self.run_origin = Some(anchor);
        self.leg_anchor = Some(anchor);
    }

    /// Advance leg anchor after a committed split (distance is absolute odometer).
    pub fn commit_leg(&mut self, anchor: TimingAnchor) {
        if self.run_origin.is_none() {
            self.run_origin = Some(anchor);
        }
        self.leg_anchor = Some(anchor);
    }

    pub fn run_sim_sec(&self, packet_id: i32) -> Option<f64> {
        let t0 = self.run_origin?.packet_id;
        Some(packet_id_delta_sec(t0, packet_id, self.physics_hz))
    }

    pub fn leg_sim_sec(&self, packet_id: i32) -> Option<f64> {
        let from = self.leg_anchor?.packet_id;
        Some(packet_id_delta_sec(from, packet_id, self.physics_hz))
    }

    pub fn leg_wall_sec(&self, now: Instant) -> Option<f64> {
        let t0 = self.leg_anchor?.at;
        Some(now.duration_since(t0).as_secs_f64())
    }

    pub fn leg_distance_m(&self, distance_traveled_m: f64) -> Option<f64> {
        let d0 = self.leg_anchor?.distance_traveled_m;
        Some((distance_traveled_m - d0).max(0.0))
    }

    /// Primary leg duration: Δpacket_id/hz; fallback to wall if sim delta is tiny.
    pub fn leg_duration_sim_and_wall(&self, packet_id: i32, now: Instant) -> Option<(f64, f64)> {
        let sim = self.leg_sim_sec(packet_id)?;
        let wall = self.leg_wall_sec(now)?;
        if sim <= 0.05 && wall > sim {
            Some((wall, wall))
        } else {
            Some((sim, wall))
        }
    }

    /// Apply optional stall correction (wall excess, see `timing_frame_quality`).
    pub fn finalize_leg_sec(
        leg_sim_sec: f64,
        leg_wall_sec: f64,
        excess_wall_sec: f64,
        apply_excess_correction: bool,
    ) -> f64 {
        let recorded = if leg_sim_sec > 0.05 {
            leg_sim_sec
        } else {
            leg_wall_sec
        };
        if apply_excess_correction && excess_wall_sec > 0.001 {
            TimingFrameMonitor::corrected_stage_leg_sec(recorded, excess_wall_sec).0
        } else {
            recorded.max(0.05)
        }
    }

    pub fn leg_duration_sec(
        &self,
        packet_id: i32,
        now: Instant,
        excess_wall_sec: f64,
        apply_excess_correction: bool,
    ) -> Option<(f64, f64)> {
        let (sim, wall) = self.leg_duration_sim_and_wall(packet_id, now)?;
        let dt = Self::finalize_leg_sec(sim, wall, excess_wall_sec, apply_excess_correction);
        Some((dt, sim))
    }
}

fn packet_id_delta_sec(from: i32, to: i32, physics_hz: f64) -> f64 {
    if to >= from {
        return (to - from) as f64 / physics_hz;
    }
    // Wrap / session reset: treat as zero-length (caller may re-anchor).
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leg_sim_from_packet_delta() {
        let t0 = Instant::now();
        let mut clock = RunTimingClock::new(333.0);
        clock.arm_run(TimingAnchor::new(1000, t0, 5000.0, None));
        let dt = clock.leg_sim_sec(1333).unwrap();
        assert!((dt - 1.0).abs() < 0.01, "dt={dt}");
    }

    #[test]
    fn distance_is_delta_not_zero_based() {
        let mut clock = RunTimingClock::new(333.0);
        clock.arm_run(TimingAnchor::new(0, Instant::now(), 12_345.0, None));
        assert!((clock.leg_distance_m(12_500.0).unwrap() - 155.0).abs() < 0.01);
    }
}
