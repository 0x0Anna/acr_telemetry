//! Report speed + position at segment times (movement-start physics lookup vs graphics reference).

use std::env;
use std::path::PathBuf;

use acr_recorder::export::rkyv_reader::read_graphics_rkyv;
use acr_recorder::recording_position::{
    load_timeline, max_game_time_sec, pos_at_game_time, speed_at_game_time, wheel_center,
};

fn gfx_at_rec_sec(
    gfx: &[acr_recorder::record::GraphicsRecord],
    rec_sec: f64,
    hz_eff: f64,
) -> Option<(f64, f64, f64, f64)> {
    if gfx.is_empty() {
        return None;
    }
    let idx_f = (rec_sec * hz_eff).clamp(0.0, (gfx.len() - 1) as f64);
    let i0 = idx_f.floor() as usize;
    let i1 = (i0 + 1).min(gfx.len() - 1);
    let f = idx_f - i0 as f64;
    let l = |a: f32, b: f32| -> f64 { a as f64 + (b as f64 - a as f64) * f };
    Some((
        l(gfx[i0].car_coordinates_x, gfx[i1].car_coordinates_x),
        l(gfx[i0].car_coordinates_y, gfx[i1].car_coordinates_y),
        l(gfx[i0].car_coordinates_z, gfx[i1].car_coordinates_z),
        l(gfx[i0].distance_traveled, gfx[i1].distance_traveled),
    ))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let physics_path = PathBuf::from(
        env::args()
            .nth(1)
            .unwrap_or_else(|| "telemetry_raw/acc_physics_1778933210.rkyv".to_string()),
    );
    let (phy, tl) = load_timeline(&physics_path)?;
    let stem = physics_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let gfx_path = physics_path
        .parent()
        .unwrap()
        .join(format!("{stem}.graphics.rkyv"));
    let (hz_g_hdr, gfx) = read_graphics_rkyv(&gfx_path)?;
    let wall = {
        let notes = physics_path.parent().unwrap().join(format!("{stem}.notes.json"));
        std::fs::read_to_string(notes).ok().and_then(|s| {
            let v: serde_json::Value = serde_json::from_str(&s).ok()?;
            let a = chrono::DateTime::parse_from_rfc3339(v["recording_start_utc"].as_str()?).ok()?;
            let b = chrono::DateTime::parse_from_rfc3339(v["recording_end_utc"].as_str()?).ok()?;
            Some((b - a).num_milliseconds() as f64 / 1000.0)
        })
    };
    let hz_g_eff = wall.map(|w| gfx.len() as f64 / w).unwrap_or(hz_g_hdr as f64);

    let markers: Vec<(&str, f64)> = vec![
        ("Start", 0.0),
        ("S1 (user leg 2:07)", 127.133),
        ("S2 (user leg 2:18)", 265.027),
        ("S3 (user leg 1:55)", 379.647),
        ("Finish (user leg 1:40)", 479.799),
        ("---", -1.0),
        ("S1 (geojson 1:40.6)", 100.591),
        ("S2 (geojson cum)", 221.659),
        ("S3 (geojson cum)", 333.017),
        ("Finish (geojson 6:55.5)", 415.538),
    ];

    println!(
        "movement: rec_t={:.2}s phy[{}] | eff_hz_phy={:.2} eff_hz_gfx={:.2} | max_game_t={:.1}s\n",
        tl.movement_phy_sec,
        tl.movement_phy_idx,
        tl.hz_p_eff,
        hz_g_eff,
        max_game_time_sec(&tl)
    );
    println!(
        "{:<22} {:>8} {:>8} {:>7} {:>10} {:>10} {:>8} {:>8} {:>8}",
        "marker", "t_game", "rec_t", "phy_i", "wheel X", "wheel Z", "spd", "gfx_spd*", "dist_m"
    );
    println!("{}", "-".repeat(100));

    for (label, t_game) in markers {
        if t_game < 0.0 {
            println!();
            continue;
        }
        let rec_t = tl.movement_phy_sec + t_game;
        let phy_i = (tl.movement_phy_idx as f64 + t_game * tl.hz_p_eff).round() as usize;
        let (x, _y, z) = pos_at_game_time(&phy, &tl, t_game).unwrap_or((0.0, 0.0, 0.0));
        let spd = speed_at_game_time(&phy, &tl, t_game);
        let wc_valid = wheel_center(&phy[phy_i.min(phy.len() - 1)]).is_some();

        let gfx_spd = if phy_i < phy.len() {
            phy[phy_i].speed_kmh
        } else {
            0.0
        };
        let dist_m = gfx_at_rec_sec(&gfx, rec_t, hz_g_eff)
            .map(|t| t.3)
            .unwrap_or(0.0);

        println!(
            "{:<22} {:8.2} {:8.2} {:7} {:10.0} {:10.0} {:8.1} {:8.1} {:8.0}",
            label,
            t_game,
            rec_t,
            phy_i,
            x,
            z,
            spd,
            gfx_spd,
            dist_m
        );
        if !wc_valid && t_game > 400.0 {
            println!("  ^ tyre_contact invalid at this index");
        }
    }

    println!("\n* rec_t = movement + t_game; spd = physics at phy index");
    Ok(())
}
