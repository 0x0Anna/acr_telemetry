//! Plausibility checks between consecutive ACC physics samples (no game clock).
//!
//! Compares wall-clock spacing, `packet_id` steps, wheel displacement, and `local_velocity`.

use std::time::Instant;

use acc_shared_memory_rs::maps::PhysicsMap;

use crate::physics_wheel;

#[derive(Debug, Clone, Copy)]
pub struct TimingQualityConfig {
    pub enabled: bool,
    /// Physics update rate for packet_id → simulation time (ACC ~333 Hz).
    pub physics_hz: f64,
    /// Subtract accumulated wall−sim excess from stage leg durations.
    pub apply_leg_excess_correction: bool,
    /// Log when wall Δt between physics frames exceeds this (seconds).
    pub max_wall_dt_sec: f64,
    /// |Δpos − v·Δt| above this (metres) flags a tick (after slop).
    pub pos_vel_slop_m: f64,
    /// Minimum relative error before flagging (fraction of expected distance).
    pub pos_vel_rel_slop: f64,
    /// Do not flag mismatch below this wall Δt (seconds).
    pub min_wall_dt_sec: f64,
    /// Min time between identical suspect logs (seconds).
    pub log_cooldown_sec: f64,
    /// Per-tick `[timing-suspect]` stderr lines (very noisy; off by default).
    pub log_suspect_ticks: bool,
}

impl Default for TimingQualityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            physics_hz: 333.0,
            apply_leg_excess_correction: false,
            max_wall_dt_sec: 0.08,
            pos_vel_slop_m: 1.5,
            pos_vel_rel_slop: 0.45,
            min_wall_dt_sec: 0.001,
            log_cooldown_sec: 0.25,
            log_suspect_ticks: false,
        }
    }
}

#[derive(Debug, Clone)]
struct PhysicsTick {
    at: Instant,
    packet_id: i32,
    wheel_x: f64,
    wheel_z: f64,
    lv_x: f32,
    lv_y: f32,
    lv_z: f32,
    speed_kmh: f32,
}

#[derive(Debug, Clone, Default)]
pub struct TickCheck {
    pub dt_wall_sec: f64,
    pub packet_delta: i32,
    pub dist_m: f64,
    pub lv_prev: (f32, f32, f32),
    pub lv_curr: (f32, f32, f32),
    pub speed_prev_mps: f64,
    pub speed_curr_mps: f64,
    pub expected_dist_m: f64,
    pub implied_speed_mps: f64,
    pub flags: Vec<&'static str>,
}

impl TickCheck {
    pub fn is_suspect(&self) -> bool {
        !self.flags.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
pub struct LegIntervalReport {
    pub wall_sec: f64,
    pub suspect_wall_sec: f64,
    /// Σ max(0, Δt_wall − Δpacket_id/physics_hz) over the leg.
    pub excess_wall_sec: f64,
    pub tick_count: u32,
    pub suspect_ticks: u32,
}

/// Raw wall time not explained by physics packet steps between two reads.
pub fn wall_excess_sec(dt_wall_sec: f64, packet_delta: i32, physics_hz: f64) -> f64 {
    if !(physics_hz > 0.0) {
        return 0.0;
    }
    if packet_delta <= 0 {
        return dt_wall_sec.max(0.0);
    }
    (dt_wall_sec - (packet_delta as f64 / physics_hz)).max(0.0)
}

/// Excess that may inflate `Instant` leg time — not routine `packet_skip` (~10 ms, pkt+3).
pub fn tick_timing_excess_sec(
    dt_wall_sec: f64,
    packet_delta: i32,
    cfg: &TimingQualityConfig,
) -> f64 {
    let raw = wall_excess_sec(dt_wall_sec, packet_delta, cfg.physics_hz);
    if packet_delta <= 0 {
        return raw;
    }
    // Catch-up reads: sim time ≈ wall; only count clear stall beyond packets.
    if dt_wall_sec > cfg.max_wall_dt_sec && raw > 0.05 {
        return raw;
    }
    0.0
}

/// Rolling physics-tick monitor for stall / motion mismatch detection.
#[derive(Debug, Clone)]
pub struct TimingFrameMonitor {
    cfg: TimingQualityConfig,
    prev: Option<PhysicsTick>,
    last_log: Option<Instant>,
    leg_wall_sec: f64,
    leg_suspect_wall_sec: f64,
    leg_ticks: u32,
    leg_suspect_ticks: u32,
}

impl TimingFrameMonitor {
    pub fn new(cfg: TimingQualityConfig) -> Self {
        Self {
            cfg,
            prev: None,
            last_log: None,
            leg_wall_sec: 0.0,
            leg_suspect_wall_sec: 0.0,
            leg_ticks: 0,
            leg_suspect_ticks: 0,
        }
    }

    pub fn reset_leg_accumulator(&mut self) {
        self.leg_wall_sec = 0.0;
        self.leg_suspect_wall_sec = 0.0;
        self.leg_ticks = 0;
        self.leg_suspect_ticks = 0;
    }

    /// `recorded_sec` from Instant anchor→cross; returns (corrected, excess_subtracted).
    pub fn corrected_stage_leg_sec(recorded_sec: f64, leg_excess_wall_sec: f64) -> (f64, f64) {
        let corrected = (recorded_sec - leg_excess_wall_sec).max(0.05);
        (corrected, leg_excess_wall_sec)
    }

    pub fn observe_physics(&mut self, physics: &PhysicsMap) -> Option<String> {
        if !self.cfg.enabled {
            return None;
        }
        let (wx, wz) = physics_wheel::wheel_center(physics)
            .map(|(x, _, z)| (x, z))
            .unwrap_or((f64::NAN, f64::NAN));
        let curr = PhysicsTick {
            at: Instant::now(),
            packet_id: physics.packet_id,
            wheel_x: wx,
            wheel_z: wz,
            lv_x: physics.local_velocity.x,
            lv_y: physics.local_velocity.y,
            lv_z: physics.local_velocity.z,
            speed_kmh: physics.speed_kmh,
        };
        let Some(prev) = self.prev.replace(curr.clone()) else {
            return None;
        };

        let check = evaluate_tick(&prev, &curr, &self.cfg);
        self.leg_ticks += 1;
        self.leg_wall_sec += check.dt_wall_sec;
        if check.is_suspect() {
            self.leg_suspect_ticks += 1;
            self.leg_suspect_wall_sec += check.dt_wall_sec;
        }

        if !check.is_suspect() {
            return None;
        }
        if !self.cfg.log_suspect_ticks {
            return None;
        }

        let line = format_tick_log(&check);

        if self
            .last_log
            .map(|t| curr.at.duration_since(t).as_secs_f64() < self.cfg.log_cooldown_sec)
            .unwrap_or(false)
        {
            return None;
        }
        self.last_log = Some(curr.at);
        Some(line)
    }

    /// Large wheel teleport between reads (crash / respawn), not normal catch-up.
    pub fn tick_position_reset_suspect(&mut self, physics: &PhysicsMap) -> bool {
        if !self.cfg.enabled {
            return false;
        }
        let (wx, wz) = physics_wheel::wheel_center(physics)
            .map(|(x, _, z)| (x, z))
            .unwrap_or((f64::NAN, f64::NAN));
        let curr = PhysicsTick {
            at: Instant::now(),
            packet_id: physics.packet_id,
            wheel_x: wx,
            wheel_z: wz,
            lv_x: physics.local_velocity.x,
            lv_y: physics.local_velocity.y,
            lv_z: physics.local_velocity.z,
            speed_kmh: physics.speed_kmh,
        };
        let Some(prev) = self.prev.as_ref() else {
            return false;
        };
        let check = evaluate_tick(prev, &curr, &self.cfg);
        if check.packet_delta > 1 {
            return false;
        }
        check.dt_wall_sec > self.cfg.max_wall_dt_sec
            && check.dist_m.is_finite()
            && check.dist_m > 15.0
    }

    /// Excess on this physics tick for stage-leg correction (per-session sum in app).
    pub fn tick_timing_excess(&mut self, physics: &PhysicsMap) -> f64 {
        if !self.cfg.enabled || !self.cfg.apply_leg_excess_correction {
            return 0.0;
        }
        let (wx, wz) = physics_wheel::wheel_center(physics)
            .map(|(x, _, z)| (x, z))
            .unwrap_or((f64::NAN, f64::NAN));
        let curr = PhysicsTick {
            at: Instant::now(),
            packet_id: physics.packet_id,
            wheel_x: wx,
            wheel_z: wz,
            lv_x: physics.local_velocity.x,
            lv_y: physics.local_velocity.y,
            lv_z: physics.local_velocity.z,
            speed_kmh: physics.speed_kmh,
        };
        let Some(prev) = self.prev.as_ref() else {
            return 0.0;
        };
        let check = evaluate_tick(prev, &curr, &self.cfg);
        tick_timing_excess_sec(check.dt_wall_sec, check.packet_delta, &self.cfg)
    }

    pub fn leg_interval_report(&self) -> LegIntervalReport {
        LegIntervalReport {
            wall_sec: self.leg_wall_sec,
            suspect_wall_sec: self.leg_suspect_wall_sec,
            excess_wall_sec: 0.0,
            tick_count: self.leg_ticks,
            suspect_ticks: self.leg_suspect_ticks,
        }
    }

    /// After a sector leg closes: one summary if the interval had suspect ticks or excess.
    pub fn leg_close_summary(
        &self,
        context: &str,
        recorded_leg_sec: f64,
        corrected_leg_sec: Option<f64>,
        leg_excess_wall_sec: f64,
    ) -> Option<String> {
        if !self.cfg.enabled {
            return None;
        }
        if self.leg_suspect_ticks == 0 && leg_excess_wall_sec < 0.001 {
            return None;
        }
        let corr = corrected_leg_sec
            .filter(|_| self.cfg.apply_leg_excess_correction && leg_excess_wall_sec > 0.001)
            .map(|c| format!(" corrected={c:.3}s"))
            .unwrap_or_default();
        Some(format!(
            "{context}: recorded_leg={recorded_leg_sec:.3}s{corr} stall_excess≈{leg_excess_wall_sec:.3}s sampled_wall={:.3}s suspect_sampled≈{:.3}s ({} suspect / {} ticks)",
            self.leg_wall_sec,
            self.leg_suspect_wall_sec,
            self.leg_suspect_ticks,
            self.leg_ticks,
        ))
    }
}

fn local_speed_mps(lv_x: f32, lv_y: f32, lv_z: f32) -> f64 {
    let x = lv_x as f64;
    let y = lv_y as f64;
    let z = lv_z as f64;
    (x * x + y * y + z * z).sqrt()
}

fn horizontal_speed_mps(lv_x: f32, lv_z: f32) -> f64 {
    let x = lv_x as f64;
    let z = lv_z as f64;
    (x * x + z * z).sqrt()
}

fn evaluate_tick(prev: &PhysicsTick, curr: &PhysicsTick, cfg: &TimingQualityConfig) -> TickCheck {
    let dt = curr.at.duration_since(prev.at).as_secs_f64();
    let packet_delta = if curr.packet_id >= prev.packet_id {
        curr.packet_id - prev.packet_id
    } else {
        i32::MAX
    };
    let dist = if prev.wheel_x.is_finite() && curr.wheel_x.is_finite() {
        let dx = curr.wheel_x - prev.wheel_x;
        let dz = curr.wheel_z - prev.wheel_z;
        (dx * dx + dz * dz).sqrt()
    } else {
        f64::NAN
    };

    let speed_prev = local_speed_mps(prev.lv_x, prev.lv_y, prev.lv_z);
    let speed_curr = local_speed_mps(curr.lv_x, curr.lv_y, curr.lv_z);
    let v_avg = 0.5 * (speed_prev + speed_curr);
    let expected_dist = v_avg * dt;
    let implied_speed = if dt > cfg.min_wall_dt_sec {
        dist / dt
    } else {
        f64::NAN
    };

    let mut flags = Vec::new();
    if dt > cfg.max_wall_dt_sec {
        flags.push("wall_gap");
    }
    if packet_delta > 1 {
        flags.push("packet_skip");
    }
    if dt >= cfg.min_wall_dt_sec && dist.is_finite() {
        let err = (dist - expected_dist).abs();
        let rel = if expected_dist > 0.05 {
            err / expected_dist
        } else if dist > cfg.pos_vel_slop_m {
            1.0
        } else {
            0.0
        };
        if err > cfg.pos_vel_slop_m && rel > cfg.pos_vel_rel_slop {
            flags.push("vel_pos_mismatch");
        }
        if dist < 0.05 && v_avg > 1.0 {
            flags.push("frozen_pos");
        }
        if dist > cfg.pos_vel_slop_m && v_avg < 0.5 && curr.speed_kmh > 3.0 {
            flags.push("ghost_move");
        }
        let h_prev = horizontal_speed_mps(prev.lv_x, prev.lv_z);
        let h_curr = horizontal_speed_mps(curr.lv_x, curr.lv_z);
        let h_avg = 0.5 * (h_prev + h_curr);
        let expected_h = h_avg * dt;
        if (dist - expected_h).abs() > cfg.pos_vel_slop_m
            && (dist - expected_h).abs() / expected_h.max(0.05) > cfg.pos_vel_rel_slop
            && !flags.contains(&"vel_pos_mismatch")
        {
            flags.push("vel_pos_xz_mismatch");
        }
    }

    TickCheck {
        dt_wall_sec: dt,
        packet_delta,
        dist_m: dist,
        lv_prev: (prev.lv_x, prev.lv_y, prev.lv_z),
        lv_curr: (curr.lv_x, curr.lv_y, curr.lv_z),
        speed_prev_mps: speed_prev,
        speed_curr_mps: speed_curr,
        expected_dist_m: expected_dist,
        implied_speed_mps: implied_speed,
        flags,
    }
}

fn format_tick_log(c: &TickCheck) -> String {
    let flags = if c.flags.is_empty() {
        "ok".to_string()
    } else {
        c.flags.join(",")
    };
    let (pvx, pvy, pvz) = c.lv_prev;
    let (cvx, cvy, cvz) = c.lv_curr;
    format!(
        "[timing-suspect] dt_wall={:.1}ms pkt+{} dist={:.2}m v_prev=({:.1},{:.1},{:.1}) v_curr=({:.1},{:.1},{:.1}) m/s expected_dist={:.2}m implied={:.1}m/s [{flags}]",
        c.dt_wall_sec * 1000.0,
        c.packet_delta,
        c.dist_m,
        pvx,
        pvy,
        pvz,
        cvx,
        cvy,
        cvz,
        c.expected_dist_m,
        c.implied_speed_mps,
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn tick(packet_id: i32, x: f64, z: f64, lv: (f32, f32, f32)) -> PhysicsTick {
        PhysicsTick {
            at: Instant::now(),
            packet_id,
            wheel_x: x,
            wheel_z: z,
            lv_x: lv.0,
            lv_y: lv.1,
            lv_z: lv.2,
            speed_kmh: (lv.0 * lv.0 + lv.2 * lv.2).sqrt() as f32 * 3.6,
        }
    }

    #[test]
    fn wall_excess_catch_up_near_zero() {
        let ex = wall_excess_sec(0.874, 291, 333.0);
        assert!(ex < 0.002, "excess={ex}");
    }

    #[test]
    fn wall_excess_pause_is_full_dt() {
        let ex = wall_excess_sec(0.5, 0, 333.0);
        assert!((ex - 0.5).abs() < 1e-9);
    }

    #[test]
    fn wall_excess_normal_skip_small() {
        let ex = wall_excess_sec(0.0105, 4, 333.0);
        assert!(ex < 0.002, "excess={ex}");
    }

    #[test]
    fn tick_timing_excess_ignores_routine_skip() {
        let cfg = TimingQualityConfig::default();
        let ex = tick_timing_excess_sec(0.0105, 4, &cfg);
        assert!(ex < 1e-6, "excess={ex}");
    }

    #[test]
    fn tick_timing_excess_counts_pause() {
        let cfg = TimingQualityConfig {
            apply_leg_excess_correction: true,
            ..Default::default()
        };
        let ex = tick_timing_excess_sec(0.5, 0, &cfg);
        assert!((ex - 0.5).abs() < 1e-9);
    }

    #[test]
    fn tick_timing_excess_ignores_catch_up_wall_gap() {
        let cfg = TimingQualityConfig::default();
        let ex = tick_timing_excess_sec(0.874, 291, &cfg);
        assert!(ex < 0.002, "excess={ex}");
    }

    #[test]
    fn flags_wall_gap() {
        let cfg = TimingQualityConfig {
            max_wall_dt_sec: 0.05,
            ..Default::default()
        };
        let mut prev = tick(1, 0.0, 0.0, (10.0, 0.0, 5.0));
        let mut curr = tick(2, 0.03, 0.015, (10.0, 0.0, 5.0));
        prev.at = Instant::now() - Duration::from_millis(200);
        curr.at = Instant::now();
        let c = evaluate_tick(&prev, &curr, &cfg);
        assert!(c.flags.contains(&"wall_gap"));
    }

    #[test]
    fn consistent_motion_ok() {
        let cfg = TimingQualityConfig::default();
        let mut prev = tick(1, 0.0, 0.0, (10.0, 0.0, 0.0));
        let mut curr = tick(2, 0.03, 0.0, (10.0, 0.0, 0.0));
        prev.at = Instant::now() - Duration::from_millis(3);
        curr.at = Instant::now();
        let c = evaluate_tick(&prev, &curr, &cfg);
        assert!(!c.is_suspect());
    }
}
