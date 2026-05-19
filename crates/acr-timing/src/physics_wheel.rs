//! Physics tyre-contact centroid (live + helpers).

use std::time::{Duration, Instant};

use acc_shared_memory_rs::maps::PhysicsMap;

use crate::stage_timing_config::StageTimingConfig;

pub fn wheel_center(p: &PhysicsMap) -> Option<(f64, f64, f64)> {
    let t = &p.tyre_contact_point;
    let x = (t.front_left.x + t.front_right.x + t.rear_left.x + t.rear_right.x) as f64 / 4.0;
    let y = (t.front_left.y + t.front_right.y + t.rear_left.y + t.rear_right.y) as f64 / 4.0;
    let z = (t.front_left.z + t.front_right.z + t.rear_left.z + t.rear_right.z) as f64 / 4.0;
    if x.abs() < 0.5 && z.abs() < 0.5 {
        return None;
    }
    Some((x, y, z))
}

#[derive(Debug, Clone, Default)]
pub struct StillstandLogState {
    last_log: Option<Instant>,
}

#[derive(Debug, Clone)]
pub struct StillstandLogContext<'a> {
    pub graphics_x: f64,
    pub graphics_z: f64,
    pub graphics_clock: f64,
    pub distance_traveled_m: f64,
    pub stage_armed: bool,
    pub stage_leg_elapsed_sec: Option<f64>,
    pub stage_next_label: Option<&'a str>,
}

/// Log wheel + graphics position while the car is nearly stopped (for calibration checks).
pub fn maybe_log_stillstand_position(
    cfg: &StageTimingConfig,
    state: &mut StillstandLogState,
    physics: &PhysicsMap,
    speed_kmh: f64,
    ctx: StillstandLogContext<'_>,
) {
    if !cfg.stillstand_position_log() {
        return;
    }
    if speed_kmh > cfg.stillstand_max_speed_kmh() {
        return;
    }
    let interval = Duration::from_secs_f64(cfg.stillstand_log_interval_sec());
    if let Some(last) = state.last_log {
        if last.elapsed() < interval {
            return;
        }
    }
    state.last_log = Some(Instant::now());

    let wheel = wheel_center(physics);
    let (wx, wy, wz) = wheel.unwrap_or((f64::NAN, f64::NAN, f64::NAN));
    let (map_z, map_x) = acr_telemetry::gis::game_xz_to_file(wx, wz);
    let leg = ctx
        .stage_leg_elapsed_sec
        .map(|t| format!("{t:.2}s"))
        .unwrap_or_else(|| "-".to_string());
    let next = ctx.stage_next_label.unwrap_or("-");
    eprintln!(
        "[stillstand] spd={speed_kmh:.1} wheel=({wx:.1},{wy:.1},{wz:.1}) map(Z,X)=({map_z:.1},{map_x:.1}) | gfx=({:.1},{:.1}) dist={:.0}m stage_armed={} leg_t={leg} next={next}",
        ctx.graphics_x,
        ctx.graphics_z,
        ctx.distance_traveled_m,
        ctx.stage_armed,
    );
}

/// Log vehicle position when a stage timing gate / sector marker is triggered.
pub fn log_sector_crossing(
    marker_label: &str,
    marker_role: &str,
    pass_method: &str,
    physics: &PhysicsMap,
    graphics_x: f64,
    graphics_z: f64,
    speed_kmh: f64,
    leg_duration_sec: Option<f64>,
    distance_traveled_m: f32,
    marker_x: f64,
    marker_z: f64,
) {
    let wheel = wheel_center(physics);
    let (wx, wy, wz) = wheel.unwrap_or((f64::NAN, f64::NAN, f64::NAN));
    let (map_z, map_x) = acr_telemetry::gis::game_xz_to_file(wx, wz);
    let leg = leg_duration_sec
        .map(|t| format!("{t:.3}s"))
        .unwrap_or_else(|| "-".to_string());
    let dist_to_marker = ((wx - marker_x).powi(2) + (wz - marker_z).powi(2)).sqrt();
    eprintln!(
        "[stage-cross] {marker_label} ({marker_role}) via={pass_method} leg={leg} spd={speed_kmh:.1} | wheel=({wx:.1},{wy:.1},{wz:.1}) map(Z,X)=({map_z:.1},{map_x:.1}) | gfx=({graphics_x:.1},{graphics_z:.1}) dist={:.0}m | d_marker={dist_to_marker:.1}m (radius only for Start)",
        distance_traveled_m
    );
}
