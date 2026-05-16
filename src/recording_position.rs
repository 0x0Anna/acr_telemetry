//! Position lookup from physics tyre contact points, timed from movement start.

use std::path::Path;

use crate::export::rkyv_reader::{read_graphics_rkyv, read_rkyv};
use crate::record::PhysicsRecord;

pub const MOVE_THRESHOLD_M: f64 = 1.0;

/// Loaded recording streams + movement anchor on the physics timeline.
pub struct RecordingTimeline {
    pub hz_p_header: u32,
    pub hz_g_header: u32,
    pub hz_p_eff: f64,
    pub phy_len: usize,
    pub gfx_len: usize,
    /// Physics sample index where the car has moved `MOVE_THRESHOLD_M` from the first valid wheel center.
    pub movement_phy_idx: usize,
    /// Recording time (seconds) of that anchor: `movement_phy_idx / hz_p_eff`.
    pub movement_phy_sec: f64,
    pub movement_x: f64,
    pub movement_y: f64,
    pub movement_z: f64,
}

pub fn wheel_center(p: &PhysicsRecord) -> Option<(f64, f64, f64)> {
    let t = &p.tyre_contact_point;
    let x = (t.front_left.x + t.front_right.x + t.rear_left.x + t.rear_right.x) as f64 / 4.0;
    let y = (t.front_left.y + t.front_right.y + t.rear_left.y + t.rear_right.y) as f64 / 4.0;
    let z = (t.front_left.z + t.front_right.z + t.rear_left.z + t.rear_right.z) as f64 / 4.0;
    // Tyre contact goes to (0,0,0) when stopped — treat as invalid.
    if x.abs() < 0.5 && z.abs() < 0.5 {
        return None;
    }
    Some((x, y, z))
}

pub fn dist_xz(ax: f64, az: f64, bx: f64, bz: f64) -> f64 {
    let dx = ax - bx;
    let dz = az - bz;
    (dx * dx + dz * dz).sqrt()
}

fn first_valid_wheel(phy: &[PhysicsRecord]) -> Option<(f64, f64, f64)> {
    phy.iter().find_map(wheel_center)
}

/// First moment the wheel centroid is `MOVE_THRESHOLD_M` from the first valid contact point.
pub fn find_movement_start_phy(phy: &[PhysicsRecord], hz: f64) -> (f64, usize, f64, f64, f64) {
    let Some((ax, ay, az)) = first_valid_wheel(phy) else {
        return (0.0, 0, 0.0, 0.0, 0.0);
    };
    if phy.len() < 2 {
        return (0.0, 0, ax, ay, az);
    }
    for i in 1..phy.len() {
        let Some((bx, by, bz)) = wheel_center(&phy[i]) else {
            continue;
        };
        let Some((px, py, pz)) = wheel_center(&phy[i - 1]) else {
            continue;
        };
        let d0 = dist_xz(ax, az, px, pz);
        let d1 = dist_xz(ax, az, bx, bz);
        if d1 >= MOVE_THRESHOLD_M {
            let frac = if (d1 - d0).abs() < 1e-9 {
                1.0
            } else {
                ((MOVE_THRESHOLD_M - d0) / (d1 - d0)).clamp(0.0, 1.0)
            };
            let x = px + (bx - px) * frac;
            let y = py + (by - py) * frac;
            let z = pz + (bz - pz) * frac;
            let t = ((i - 1) as f64 + frac) / hz;
            return (t, i, x, y, z);
        }
    }
    let last = wheel_center(phy.last().unwrap()).unwrap_or((ax, ay, az));
    (
        (phy.len() - 1) as f64 / hz,
        phy.len() - 1,
        last.0,
        last.1,
        last.2,
    )
}

fn wall_secs_from_notes(notes_path: &Path) -> Option<f64> {
    let s = std::fs::read_to_string(notes_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let a = chrono::DateTime::parse_from_rfc3339(v["recording_start_utc"].as_str()?).ok()?;
    let b = chrono::DateTime::parse_from_rfc3339(v["recording_end_utc"].as_str()?).ok()?;
    Some((b - a).num_milliseconds() as f64 / 1000.0)
}

pub fn effective_physics_hz(phy_len: usize, notes_path: &Path) -> f64 {
    if let Some(w) = wall_secs_from_notes(notes_path) {
        if w > 1.0 {
            return phy_len as f64 / w;
        }
    }
    333.0
}

pub fn load_timeline(physics_path: &Path) -> Result<(Vec<PhysicsRecord>, RecordingTimeline), Box<dyn std::error::Error>> {
    let stem = physics_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let notes_path = physics_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}.notes.json"));

    let (hz_p_header, phy) = read_rkyv(physics_path)?;
    let hz_p_eff = effective_physics_hz(phy.len(), &notes_path);
    let (move_sec, move_idx, mx, my, mz) = find_movement_start_phy(&phy, hz_p_eff);

    let gfx_path = physics_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}.graphics.rkyv"));
    let (hz_g_header, gfx) = read_graphics_rkyv(&gfx_path).unwrap_or((60, Vec::new()));

    let timeline = RecordingTimeline {
        hz_p_header,
        hz_g_header: hz_g_header,
        hz_p_eff,
        phy_len: phy.len(),
        gfx_len: gfx.len(),
        movement_phy_idx: move_idx,
        movement_phy_sec: move_sec,
        movement_x: mx,
        movement_y: my,
        movement_z: mz,
    };
    Ok((phy, timeline))
}

/// Wheel centroid at `t_game_sec` after **movement start** (stage timing zero).
pub fn pos_at_game_time(phy: &[PhysicsRecord], tl: &RecordingTimeline, t_game_sec: f64) -> Option<(f64, f64, f64)> {
    if phy.is_empty() {
        return None;
    }
    let idx_f = (tl.movement_phy_idx as f64 + t_game_sec * tl.hz_p_eff).clamp(0.0, (phy.len() - 1) as f64);
    let i0 = idx_f.floor() as usize;
    let i1 = (i0 + 1).min(phy.len() - 1);
    let frac = idx_f - i0 as f64;
    let p0 = wheel_center(&phy[i0])?;
    let p1 = wheel_center(&phy[i1]).unwrap_or(p0);
    let lerp = |a: f64, b: f64| a + (b - a) * frac;
    Some((lerp(p0.0, p1.0), lerp(p0.1, p1.1), lerp(p0.2, p1.2)))
}

pub fn speed_at_game_time(phy: &[PhysicsRecord], tl: &RecordingTimeline, t_game_sec: f64) -> f32 {
    if phy.is_empty() {
        return 0.0;
    }
    let idx_f = (tl.movement_phy_idx as f64 + t_game_sec * tl.hz_p_eff)
        .clamp(0.0, (phy.len() - 1) as f64);
    let i0 = idx_f.floor() as usize;
    let i1 = (i0 + 1).min(phy.len() - 1);
    let frac = (idx_f - i0 as f64) as f32;
    let v0 = phy[i0].speed_kmh;
    let v1 = phy[i1].speed_kmh;
    v0 + (v1 - v0) * frac
}

/// Max stage time after movement start reachable in the physics file.
pub fn max_game_time_sec(tl: &RecordingTimeline) -> f64 {
    if tl.phy_len <= tl.movement_phy_idx + 1 {
        return 0.0;
    }
    (tl.phy_len - 1 - tl.movement_phy_idx) as f64 / tl.hz_p_eff
}

pub fn parse_duration(s: &str) -> Result<f64, String> {
    let s = s.trim();
    let s = if s.contains(':') {
        s.to_string()
    } else if s.matches('.').count() == 2 {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() == 3 {
            format!("{}:{}.{}", parts[0], parts[1], parts[2])
        } else {
            s.to_string()
        }
    } else {
        s.to_string()
    };
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        2 => {
            let min: f64 = parts[0].parse().map_err(|_| format!("bad minutes in {s}"))?;
            let sec: f64 = parts[1].parse().map_err(|_| format!("bad seconds in {s}"))?;
            Ok(min * 60.0 + sec)
        }
        3 => {
            let h: f64 = parts[0].parse().map_err(|_| format!("bad hours in {s}"))?;
            let min: f64 = parts[1].parse().map_err(|_| format!("bad minutes in {s}"))?;
            let sec: f64 = parts[2].parse().map_err(|_| format!("bad seconds in {s}"))?;
            Ok(h * 3600.0 + min * 60.0 + sec)
        }
        _ => Err(format!("expected M:SS.mmm, got {s}")),
    }
}
