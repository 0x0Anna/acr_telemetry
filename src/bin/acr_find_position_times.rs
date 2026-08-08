//! Find recording times when stillstand calibration positions occur in an rkyv file.

use std::env;
use std::path::PathBuf;

use acr_recorder::export::rkyv_reader::read_graphics_rkyv;
use acr_recorder::recording_position::{load_timeline, wheel_center, RecordingTimeline};

struct Target {
    label: &'static str,
    dist_m: f64,
    wx: f64,
    wz: f64,
    // Reference graphics-space coordinates, kept alongside wx/wz for
    // cross-checking against the physics-space fields when debugging by
    // hand; not read programmatically.
    #[allow(dead_code)]
    gx: f64,
    #[allow(dead_code)]
    gz: f64,
}

fn wall_hz_gfx(gfx_len: usize, notes_path: &std::path::Path) -> f64 {
    let s = std::fs::read_to_string(notes_path).ok();
    let wall = s.and_then(|s| {
        let v: serde_json::Value = serde_json::from_str(&s).ok()?;
        let a = chrono::DateTime::parse_from_rfc3339(v["recording_start_utc"].as_str()?).ok()?;
        let b = chrono::DateTime::parse_from_rfc3339(v["recording_end_utc"].as_str()?).ok()?;
        Some((b - a).num_milliseconds() as f64 / 1000.0)
    });
    wall.map(|w| gfx_len as f64 / w).unwrap_or(60.0)
}

fn first_dist_crossing(dist: &[f32], hz: f64, target: f64) -> Option<(usize, f64)> {
    for (i, &d) in dist.iter().enumerate() {
        if d as f64 >= target {
            return Some((i, i as f64 / hz));
        }
    }
    let i = dist.len().saturating_sub(1);
    Some((i, i as f64 / hz))
}

fn closest_wheel_by_xz(
    phy: &[acr_recorder::record::PhysicsRecord],
    tl: &RecordingTimeline,
    tx: f64,
    tz: f64,
) -> Option<(usize, f64, f64, f32)> {
    let mut best: Option<(usize, f64, f32)> = None;
    for (i, p) in phy.iter().enumerate() {
        let Some((x, _, z)) = wheel_center(p) else {
            continue;
        };
        let d = ((x - tx).powi(2) + (z - tz).powi(2)).sqrt();
        if best.map_or(true, |(_, bd, _)| d < bd) {
            best = Some((i, d, p.speed_kmh));
        }
    }
    best.map(|(i, _d, spd)| {
        let t_rec = i as f64 / tl.hz_p_eff;
        let t_game = (i as f64 - tl.movement_phy_idx as f64).max(0.0) / tl.hz_p_eff;
        (i, t_rec, t_game, spd)
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let physics_path = PathBuf::from(
        env::args()
            .nth(1)
            .unwrap_or_else(|| "telemetry_raw/acc_physics_1778933210.rkyv".to_string()),
    );
    let stem = physics_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let notes_path = physics_path
        .parent()
        .unwrap()
        .join(format!("{stem}.notes.json"));

    let targets = [
        Target {
            label: "Start",
            dist_m: 154.0,
            wx: 657.7,
            wz: -1087.1,
            gx: 657.7,
            gz: -1087.0,
        },
        Target {
            label: "S1",
            dist_m: 2997.0,
            wx: -993.0,
            wz: 311.9,
            gx: -993.0,
            gz: 312.1,
        },
        Target {
            label: "S2",
            dist_m: 6412.0,
            wx: 111.4,
            wz: 46.1,
            gx: 111.5,
            gz: 45.9,
        },
        Target {
            label: "S3",
            dist_m: 9587.0,
            wx: 1601.5,
            wz: -576.8,
            gx: 1601.7,
            gz: -576.8,
        },
        Target {
            label: "Finish",
            dist_m: 11764.0,
            wx: 1132.7,
            wz: 1121.8,
            gx: 1132.7,
            gz: 1122.0,
        },
    ];

    let (phy, tl) = load_timeline(&physics_path)?;
    let (_hz_g_hdr, gfx) = read_graphics_rkyv(
        physics_path
            .parent()
            .unwrap()
            .join(format!("{stem}.graphics.rkyv")),
    )?;
    let hz_g = wall_hz_gfx(gfx.len(), &notes_path);
    let dist_g: Vec<f32> = gfx.iter().map(|g| g.distance_traveled).collect();

    println!("file: {}", physics_path.display());
    println!(
        "physics {} @ eff {:.1} Hz | movement @ rec {:.2}s phy[{}] | graphics {} @ eff {:.1} Hz\n",
        phy.len(),
        tl.hz_p_eff,
        tl.movement_phy_sec,
        tl.movement_phy_idx,
        gfx.len(),
        hz_g
    );
    println!(
        "{:<8} {:>10} {:>10} {:>10} {:>8} | {:>10} {:>10} {:>8} (closest wheel XZ in physics)",
        "point", "t_rec", "t_move", "t_gfx", "spd", "t_rec", "t_move", "err_m"
    );
    println!("{}", "-".repeat(95));

    for t in targets {
        let (gi, t_gfx) = first_dist_crossing(&dist_g, hz_g, t.dist_m).unwrap_or((0, 0.0));
        let spd_gfx = if gi < phy.len() {
            let pi = ((gi as f64 / hz_g) * tl.hz_p_eff).round() as usize;
            phy[pi.min(phy.len() - 1)].speed_kmh
        } else {
            0.0
        };

        let (pi, t_rec, t_game, spd_phy) =
            closest_wheel_by_xz(&phy, &tl, t.wx, t.wz).unwrap_or((0, 0.0, 0.0, 0.0));
        let (wx, _, wz) = wheel_center(&phy[pi]).unwrap_or((0.0, 0.0, 0.0));
        let err = ((wx - t.wx).powi(2) + (wz - t.wz).powi(2)).sqrt();

        println!(
            "{:<8} {:10.2} {:10.2} {:10.2} {:8.1} | {:10.2} {:10.2} {:8.1}",
            t.label, t_gfx, (t_gfx - tl.movement_phy_sec).max(0.0), t_gfx, spd_gfx, t_rec, t_game, err
        );
        println!(
            "         dist gfx={:.0}m @ gfx[{gi}] | wheel@phy[{pi}] ({wx:.1},{wz:.1}) spd={spd_phy:.0} target dist={:.0}",
            gfx[gi].distance_traveled, t.dist_m
        );
    }

    println!("\nt_rec = seconds from recording start (sample 0)");
    println!("t_move = t_rec - movement_start ({:.2}s)", tl.movement_phy_sec);
    println!("t_gfx = graphics sample index / eff_hz_g (distance_traveled crossing)");
    Ok(())
}
