//! Verbose stderr timing diagnostics (`timing_debug` in acr_timing.toml).

use acc_shared_memory_rs::maps::PhysicsMap;

use crate::physics_wheel::wheel_center;
use crate::stage_sector_timing::stage_marker_is_main_sector;
use crate::timing_sectors::TimingSectorMarker;

/// Hauptsektor-Grenze (S1–S3, Finish) — nicht Subsektor-CPs.
pub fn is_hauptsektor_label(label: &str) -> bool {
    matches!(label, "Sector 1" | "Sector 2" | "Sector 3" | "Finish")
}

pub fn spielzeit_sec(graphics_current_time_ms: i32) -> f64 {
    graphics_current_time_ms as f64 / 1000.0
}

fn format_pos(physics: &PhysicsMap, graphics_x: f64, graphics_z: f64) -> String {
    let (wx, wy, wz) = wheel_center(physics).unwrap_or((f64::NAN, f64::NAN, f64::NAN));
    let (map_z, map_x) = crate::gis::game_xz_to_file(wx, wz);
    format!(
        "rad=({wx:.1},{wy:.1},{wz:.1}) map(Z,X)=({map_z:.1},{map_x:.1}) gfx=({graphics_x:.1},{graphics_z:.1})",
    )
}

/// Subsektor-Zeit (GeoJSON-Gate → Gate, packet_id-Uhr).
#[allow(clippy::too_many_arguments)]
pub fn log_subsektor_zeit(
    from_label: &str,
    to_label: &str,
    from_seg: i32,
    to_seg: i32,
    dt_sec: f64,
    dt_raw_sec: f64,
    subsektor_summe_sec: f64,
    zeitnahme_run_sec: Option<f64>,
    spielzeit_sec: f64,
    physics: &PhysicsMap,
    graphics_x: f64,
    graphics_z: f64,
    speed_kmh: f32,
    distance_traveled_m: f64,
    packet_id: i32,
) {
    let pos = format_pos(physics, graphics_x, graphics_z);
    let run = zeitnahme_run_sec
        .map(|t| format!("{t:.3}s"))
        .unwrap_or_else(|| "—".to_string());
    let haupt = if is_hauptsektor_label(to_label) {
        " [HAUPTSEKTOR-GRENZE]"
    } else {
        ""
    };
    eprintln!(
        "[zeitnahme] Subsektor {from_label}→{to_label} (id {from_seg}→{to_seg}) \
         dt={dt_sec:.3}s raw={dt_raw_sec:.3}s Σ_Subsektoren={subsektor_summe_sec:.3}s{haupt} | \
         {pos} spd={speed_kmh:.0} dist={distance_traveled_m:.0}m pkt={packet_id} | \
         spielzeit={spielzeit_sec:.3}s zeitnahme_run={run}",
    );
}

/// Stage-Sektor (kalibrierte timing_sectors.geojson: Start→S1, S1→S2, …).
#[allow(clippy::too_many_arguments)]
pub fn log_stage_sektor_zeit(
    marker: &TimingSectorMarker,
    pass_method: &str,
    dt_sec: f64,
    dt_raw_sec: Option<f64>,
    stage_sektoren_summe_sec: f64,
    subsektor_summe_sec: f64,
    zeitnahme_run_sec: Option<f64>,
    spielzeit_sec: f64,
    physics: &PhysicsMap,
    graphics_x: f64,
    graphics_z: f64,
    speed_kmh: f32,
    distance_traveled_m: f32,
) {
    if !stage_marker_is_main_sector(marker) {
        return;
    }
    let pos = format_pos(physics, graphics_x, graphics_z);
    let run = zeitnahme_run_sec
        .map(|t| format!("{t:.3}s"))
        .unwrap_or_else(|| "—".to_string());
    let raw = dt_raw_sec
        .map(|r| format!(" raw={r:.3}s"))
        .unwrap_or_default();
    eprintln!(
        "[zeitnahme] Stage-Sektor {} ({}) via={pass_method} dt={dt_sec:.3}s{raw} \
         Σ_Stage-Sektoren={stage_sektoren_summe_sec:.3}s Σ_Subsektoren={subsektor_summe_sec:.3}s | \
         {pos} spd={speed_kmh:.0} dist={distance_traveled_m:.0}m | \
         spielzeit={spielzeit_sec:.3}s zeitnahme_run={run}",
        marker.label,
        marker.role.as_str(),
    );
}

/// Modularer Presenter: Sektor fertig (tot = Summe Subsektoren in diesem Block, sofern vollständig).
#[allow(clippy::too_many_arguments)]
pub fn log_sektor_fertig_vergleich(
    sector_index: u32,
    sektor_tot_sec: f64,
    sektor_sub_summe_sec: f64,
    stage_sektor_sec: Option<f64>,
    subsektor_summe_gesamt_sec: f64,
    osd_track_completed_summe_sec: f64,
    zeitnahme_run_sec: Option<f64>,
    spielzeit_sec: f64,
) {
    let stage = stage_sektor_sec
        .map(|t| format!("{t:.3}s"))
        .unwrap_or_else(|| "—".to_string());
    let run = zeitnahme_run_sec
        .map(|t| format!("{t:.3}s"))
        .unwrap_or_else(|| "—".to_string());
    let diff_stage = stage_sektor_sec.map(|st| sektor_tot_sec - st);
    let diff_spiel = zeitnahme_run_sec.map(|r| r - spielzeit_sec);
    eprintln!(
        "[zeitnahme] Sektor S{} fertig | presenter_tot={sektor_tot_sec:.3}s \
         (Summe Subsektoren im Block={sektor_sub_summe_sec:.3}s) stage_sektor={stage} \
         Δ_presenter−stage={} | Σ_Subsektoren_alle_gates={subsektor_summe_gesamt_sec:.3}s \
         OSD_TrackCompleted_cum={osd_track_completed_summe_sec:.3}s | \
         spielzeit={spielzeit_sec:.3}s zeitnahme_run={run} Δ_run−spiel={}",
        sector_index + 1,
        diff_stage
            .map(|d| format!("{d:+.3}s"))
            .unwrap_or_else(|| "—".to_string()),
        diff_spiel
            .map(|d| format!("{d:+.3}s"))
            .unwrap_or_else(|| "—".to_string()),
    );
}

/// Streckenende: alle drei Summen nebeneinander.
#[allow(clippy::too_many_arguments)]
pub fn log_strecke_fertig_vergleich(
    osd_track_completed_summe_sec: f64,
    subsektor_summe_gesamt_sec: f64,
    stage_sektoren_summe_sec: f64,
    zeitnahme_run_sec: Option<f64>,
    spielzeit_sec: f64,
) {
    let run = zeitnahme_run_sec
        .map(|t| format!("{t:.3}s"))
        .unwrap_or_else(|| "—".to_string());
    eprintln!(
        "[zeitnahme] Strecke fertig | OSD_TrackCompleted_cum={osd_track_completed_summe_sec:.3}s \
         (= Summe presenter Sektor-tot, meist Subsektor-Summen pro Block; Stage-Sektor kann OSD tot ersetzen) | \
         Σ_Subsektoren_alle_gates={subsektor_summe_gesamt_sec:.3}s | \
         Σ_Stage-Sektoren={stage_sektoren_summe_sec:.3}s | \
         spielzeit={spielzeit_sec:.3}s zeitnahme_run={run} | \
         Δ_cum−subsektoren={:+.3}s Δ_cum−stage={:+.3}s Δ_run−spiel={}",
        osd_track_completed_summe_sec - subsektor_summe_gesamt_sec,
        osd_track_completed_summe_sec - stage_sektoren_summe_sec,
        zeitnahme_run_sec
            .map(|r| r - spielzeit_sec)
            .map(|d| format!("{d:+.3}s"))
            .unwrap_or_else(|| "—".to_string()),
    );
}
