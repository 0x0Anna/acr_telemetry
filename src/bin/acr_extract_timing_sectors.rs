//! Extract sector boundary positions from a calibration recording + split times.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use acr_recorder::gis;
use acr_recorder::recording_position::{
    load_timeline, max_game_time_sec, pos_at_game_time, speed_at_game_time, MOVE_THRESHOLD_M,
};
use serde_json::json;

fn parse_duration(s: &str) -> Result<f64, String> {
    let s = s.trim();
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
        _ => Err(format!("expected M:SS.mmm or H:MM:SS.mmm, got {s}")),
    }
}

#[derive(Clone)]
struct MarkerSpec {
    role: &'static str,
    order: i32,
    label: &'static str,
    description: &'static str,
    offset_from_t0_sec: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "usage: acr_extract_timing_sectors <physics.rkyv> [--out timing/timing_sectors.geojson]"
        );
        std::process::exit(1);
    }
    let physics_path = PathBuf::from(&args[1]);
    let out_path = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("timing/timing_sectors.geojson"));

    let gfx_path = graphics_sidecar_path(&physics_path);

    let (phy, tl) = load_timeline(&physics_path)?;
    if phy.is_empty() {
        return Err("no physics records".into());
    }

    let split_01 = parse_duration("1:40.591")?;
    let split_12 = parse_duration("2:01.068")?;
    let split_23 = parse_duration("1:52.358")?;
    let split_3f = parse_duration("1:21.504")?;
    let total = parse_duration("6:55.538")?;

    let t0 = tl.movement_phy_sec;
    let t0_idx = tl.movement_phy_idx;
    let t0_x = tl.movement_x;
    let t0_z = tl.movement_z;
    let max_game = max_game_time_sec(&tl);
    eprintln!(
        "lookup: movement_start + game split | anchor rec_t={t0:.3}s phy[{t0_idx}] | eff_hz={:.2} | max_game {:.3}s",
        tl.hz_p_eff, max_game
    );

    let markers = [
        MarkerSpec {
            role: "timing_start",
            order: 0,
            label: "Start",
            description: "Stage timing zero (in-game splits); wheel centroid at movement start",
            offset_from_t0_sec: 0.0,
        },
        MarkerSpec {
            role: "sector_boundary",
            order: 1,
            label: "Sector 1",
            description: "End of sector 1 (split Start→1: 1:40.591)",
            offset_from_t0_sec: split_01,
        },
        MarkerSpec {
            role: "sector_boundary",
            order: 2,
            label: "Sector 2",
            description: "End of sector 2 (split 1→2: 2:01.068)",
            offset_from_t0_sec: split_01 + split_12,
        },
        MarkerSpec {
            role: "sector_boundary",
            order: 3,
            label: "Sector 3",
            description: "End of sector 3 (split 2→3: 1:52.358)",
            offset_from_t0_sec: split_01 + split_12 + split_23,
        },
        MarkerSpec {
            role: "finish",
            order: 4,
            label: "Finish",
            description: "Finish (split 3→Finish: 1:21.504; overall Start→Finish: 6:55.538)",
            offset_from_t0_sec: total,
        },
    ];

    let mut features = Vec::new();
    for m in &markers {
        let t_game = m.offset_from_t0_sec;
        let (gx, _gy, gz) = pos_at_game_time(&phy, &tl, t_game).unwrap_or((t0_x, 0.0, t0_z));
        let spd = speed_at_game_time(&phy, &tl, t_game);
        let (file_x, file_y) = gis::game_xz_to_file(gx, gz);
        features.push(json!({
            "type": "Feature",
            "geometry": {
                "type": "Point",
                "coordinates": [file_x, file_y]
            },
            "properties": {
                "kind": format!("timing_sector_{}", m.role),
                "marker_role": m.role,
                "marker_order": m.order,
                "marker_label": m.label,
                "description": m.description,
                "stage": "Cwmbiga - Afon Biga",
                "stage_slug": "cwmbiga_afon_biga",
                "reference_track": "hafren_north",
                "time_offset_sec": (t_game * 1000.0).round() / 1000.0,
                "time_from_timing_start_sec": m.offset_from_t0_sec,
                "position_lookup_time_sec": t_game,
                "speed_kmh_at_marker": (spd * 10.0).round() / 10.0,
                "recording_lead_in_sec": t0,
                "movement_start_phy_index": t0_idx,
                "game_x": gx,
                "game_z": gz,
                "position_source": "physics_tyre_contact_centroid",
                "source_recording": physics_path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                "calibration_note": "Position = wheel centroid at movement_start + game split time; eff_hz from wall clock"
            }
        }));
    }

    let sum_splits = split_01 + split_12 + split_23 + split_3f;
    let collection = json!({
        "type": "FeatureCollection",
        "name": "timing_sectors:cwmbiga_afon_biga",
        "properties": {
            "stage": "Cwmbiga - Afon Biga",
            "stage_slug": "cwmbiga_afon_biga",
            "reference_track": "hafren_north",
            "coordinate_space": "acc_world_zx",
            "swap_xz": true,
            "qgis_overlay_with": "reference_tracks/hafren_north.shp or timing/pacenotes/*.geojson (not timing/sectors_filtered.shp — game coords)",
            "source_recording": physics_path.to_string_lossy(),
            "source_graphics": gfx_path.to_string_lossy(),
            "physics_sample_rate_hz_header": tl.hz_p_header,
            "physics_effective_hz": tl.hz_p_eff,
            "movement_threshold_m": MOVE_THRESHOLD_M,
            "recording_lead_in_sec": t0,
            "position_time_uses_game_clock": true,
            "position_source": "physics_tyre_contact_centroid",
            "timing_start_phy_index": t0_idx,
            "timing_start_game_x": t0_x,
            "timing_start_game_z": t0_z,
            "max_game_time_sec": max_game,
            "split_start_to_1_sec": split_01,
            "split_1_to_2_sec": split_12,
            "split_2_to_3_sec": split_23,
            "split_3_to_finish_sec": split_3f,
            "split_sum_sec": sum_splits,
            "split_total_sec": total,
            "recording_duration_sec": (tl.phy_len as f64) / tl.hz_p_eff,
            "marker_count": features.len()
        },
        "features": features
    });

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out_path, serde_json::to_string_pretty(&collection)? + "\n")?;
    eprintln!(
        "wrote {} (movement @ {:.3}s phy[{}], {} markers)",
        out_path.display(),
        t0,
        t0_idx,
        collection["properties"]["marker_count"]
    );
    Ok(())
}

fn graphics_sidecar_path(physics: &Path) -> PathBuf {
    let stem = physics.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    physics
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}.graphics.rkyv"))
}
