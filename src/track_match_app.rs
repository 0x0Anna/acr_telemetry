//! Live/Offline track matching against reference tracks.
//!
//! Usage examples:
//!   acr_track_match --refs A.rkyv,B.rkyv,C.points.shp --input current.rkyv
//!   acr_track_match --refs A.rkyv,B.rkyv,C.rkyv --live

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use acc_shared_memory_rs::ACCSharedMemory;
use crate::app_config;
use crate::config;
use crate::export::rkyv_reader;
use acr_pacenote::pacenote_ambiguous_overlay::{
    self as pacenote_amb_overlay, AmbiguousPacenoteOverlayState, TrackStartPickOverlayState,
};
use acr_pacenote::pacenote_course::{
    self, PacenoteCallout, PacenoteCourse, PacenoteStageCatalog, PacenoteStagePick,
};
use acr_pacenote::pacenote_voice::{PacenoteConfig, PacenoteVoicePlayer};
use acr_pacenote::win_picker_input::{PacenotePickerKeyTracker, PacenotePickerNav};
use acr_timing::sector_leg_stats::{SectorLegStatsAccumulator, SectorLegStatsSnapshot};
use acr_timing::split_beep::SplitBeepConfig;
use acr_timing::subtiming::{SectorPassEvent, SectorPassTracker, SectorTravelDirection};
use acc_shared_memory_rs::datatypes::Wheels;
use acc_shared_memory_rs::maps::PhysicsMap;
use serde::Deserialize;
use shapefile::dbase::FieldValue;

static RUNNING: AtomicBool = AtomicBool::new(true);
const SAME_SECTOR_REANCHOR_SEC: f64 = 2.5;
const START_STAGE_HOLD_SEC: f64 = 3.0;
const START_STAGE_RPM_MIN: f64 = 2000.0;
const START_STAGE_SPEED_MAX: f64 = 1.5;
const START_STAGE_RADIUS_M: f64 = 1.5;
const START_TRIGGER_SPEED_KMH: f64 = 5.0;
const START_SECTOR_ID: i32 = -1;
/// Overall stage time (pacenote start → finish), stored in `sector_splits`.
const FINISH_SECTOR_ID: i32 = -2;
const OVERALL_MARKER_RADIUS_M: f64 = 25.0;
/// Log pacenote first-anchor distances while geometry coarse-match fails (throttle).
const PACENOTE_ANCHOR_HELP_SECS: u64 = 3;
/// Flag a possible teleport when the car jumps more than this between physics frames.
const START_LAYOUT_TELEPORT_RESET_M: f64 = 30.0;
/// Unlock only after this many seconds standstill at a start grid following a jump.
const TELEPORT_UNLOCK_STILL_SEC: f64 = 3.0;
/// Clear a pending teleport unlock if the car keeps driving above standstill speed this long.
const TELEPORT_PENDING_CLEAR_DRIVE_SEC: f64 = 2.0;
/// Ignore sector line crossings for a single physics step longer than this (stale `last_pt` / menu teleport).
const MAX_SECTOR_CROSS_SEGMENT_M: f64 = 120.0;
/// Defaults when `timing/start_points.geojson` (or configured path) has anchors: track lock from
/// start-point + `track_spline_length` (see `timing/track_spline_lengths.toml`). Lock persists until
/// jump (`START_LAYOUT_TELEPORT_RESET_M`) + stillstand at start grid, or car model change.
const DEFAULT_GRID_STANDSTILL_MAX_SPEED_KMH: f64 = 2.0;
const DEFAULT_GRID_START_TRIGGER_RADIUS_M: f64 = 2.0;
const DEFAULT_GRID_START_LIST_RADIUS_INITIAL_M: f64 = 2.0;
const DEFAULT_GRID_START_WIDE_AFTER_SEC: f64 = 10.0;
const DEFAULT_GRID_START_LIST_RADIUS_WIDE_M: f64 = 100.0;

#[derive(Clone, Copy, Debug)]
struct Point2 {
    x: f64,
    z: f64,
}

fn point2_from_file(file_x: f64, file_y: f64) -> Point2 {
    let (x, z) = acr_telemetry::gis::file_to_game_xz(file_x, file_y);
    Point2 { x, z }
}

/// How sector boundary vertices are stored in the sectors shapefile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SectorsCoordSpace {
    /// GIS file convention `[game_z, game_x]` — same as `reference_tracks/*.shp` (default).
    #[default]
    File,
    /// Vertices are already ACC game XZ (`sectors_all` / `sectors_filtered` pipeline).
    Game,
}

impl SectorsCoordSpace {
    fn parse(s: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match s.trim().to_ascii_lowercase().as_str() {
            "file" | "swap_xz" | "gis" => Ok(Self::File),
            "game" | "acc" | "world" => Ok(Self::Game),
            other => Err(format!("unknown sectors_coord_space: {other} (use file or game)").into()),
        }
    }
}

fn point2_from_sectors_shp(file_x: f64, file_y: f64, space: SectorsCoordSpace) -> Point2 {
    match space {
        SectorsCoordSpace::File => point2_from_file(file_x, file_y),
        SectorsCoordSpace::Game => Point2 {
            x: file_x,
            z: file_y,
        },
    }
}

fn reset_live_route_samples(
    history: &mut VecDeque<Point2>,
    last_pt: &mut Option<Point2>,
    total_drive_m: &mut f64,
) {
    history.clear();
    *last_pt = None;
    *total_drive_m = 0.0;
}

/// Ring index of the sector boundary nearest `p` (uses boundary midpoint distance).
fn nearest_sector_ring_index(set: &SectorSet, p: Point2) -> (usize, f64) {
    let mut best_ring = 0usize;
    let mut best_d = f64::INFINITY;
    for b in &set.boundaries {
        let ring_idx = b.sector_id as usize;
        let center = Point2 {
            x: (b.a.x + b.b.x) * 0.5,
            z: (b.a.z + b.b.z) * 0.5,
        };
        let d = dist(p, center);
        if d < best_d {
            best_d = d;
            best_ring = ring_idx;
        }
    }
    (best_ring, best_d)
}

/// Set the sector tracker from car position. Line-crossing must not be used for this initial anchor.
fn seed_sector_tracker_at_position(state: &mut LiveTimingState, set: &SectorSet, p: Point2) {
    if state.tracker.current_sector().is_some() {
        return;
    }
    let (ring_idx, best_d) = nearest_sector_ring_index(set, p);
    let _ = state.tracker.observe(ring_idx);
    state.last_sector_idx = Some(ring_idx);
    let seg = set.ring_ids.get(ring_idx).copied().unwrap_or(-1);
    eprintln!(
        "sector at position: [{}] ({:.0} m to nearest boundary)",
        seg, best_d
    );
}

#[derive(Debug)]
struct ReferenceTrack {
    name: String,
    points: Vec<Point2>,
    headings: Vec<f64>,
}

#[derive(Debug)]
struct MatchScore {
    name: String,
    coarse_pass: bool,
    coarse_inlier_ratio: f64,
    mean_dist_m: f64,
    mean_heading_diff_rad: f64,
    final_score: f64,
}

#[derive(Clone, Debug)]
struct SectorBoundary {
    sector_id: i32,
    a: Point2,
    b: Point2,
}

#[derive(Clone, Debug)]
struct SectorSet {
    boundaries: Vec<SectorBoundary>,
    ring_ids: Vec<i32>,
}

#[derive(Debug)]
struct LiveTimingState {
    tracker: SectorPassTracker,
    ring_ids: Vec<i32>,
    last_anchor_t_sec: Option<f64>,
    last_anchor_instant: Option<Instant>,
    last_anchor_drive_m: Option<f64>,
    last_sector_idx: Option<usize>,
    start_stage_pos: Option<Point2>,
    start_stage_since: Option<Instant>,
    start_stage_last_report_sec: i32,
    start_armed: bool,
    start_anchor_t_sec: Option<f64>,
    start_anchor_instant: Option<Instant>,
    start_anchor_drive_m: Option<f64>,
    cooldown_until: HashMap<usize, Instant>,
    /// Pacenote-derived start/finish for Gesamtzeit (`timing/overall_markers/<slug>.geojson`).
    overall_markers: Option<acr_timing::stage_overall_markers::StageOverallMarkers>,
    overall_finish_recorded: bool,
    /// Calibrated stage sectors (`[timing.ref_stage_sectors]` → GeoJSON). Independent of pacenotes.
    stage_sector_session: Option<acr_timing::stage_sector_timing::StageSectorSession>,
    /// Physics aggregates for the current subsection leg (anchor → next cross).
    leg_stats: SectorLegStatsAccumulator,
    /// Speed (km/h) at subsection leg entry (set with anchor).
    leg_entry_speed_kmh: Option<f32>,
}

fn set_leg_entry_speed(state: &mut LiveTimingState, speed_kmh: f32) {
    state.leg_entry_speed_kmh = Some(speed_kmh);
}

fn wheels4(w: &Wheels) -> [f32; 4] {
    [
        w.front_left,
        w.front_right,
        w.rear_left,
        w.rear_right,
    ]
}

fn observe_active_leg_stats(state: &mut LiveTimingState, physics: &PhysicsMap) {
    if state.last_anchor_instant.is_some() || state.start_armed {
        state.leg_stats.observe_sample(
            physics.gas,
            wheels4(&physics.slip_ratio),
            wheels4(&physics.slip_angle),
        );
    }
}

fn take_leg_stats(state: &mut LiveTimingState, exit_speed_kmh: f32) -> Option<SectorLegStatsSnapshot> {
    let entry = state.leg_entry_speed_kmh?;
    let snapshot = state.leg_stats.finalize_leg(entry, exit_speed_kmh);
    state.leg_stats.reset();
    state.leg_entry_speed_kmh = None;
    Some(snapshot)
}

fn attach_stage_overall_markers(
    state: &mut LiveTimingState,
    pacenote_geojson: Option<&Path>,
    cache: &mut acr_timing::stage_overall_markers::MarkerCache,
) {
    state.overall_markers = pacenote_geojson
        .and_then(|p| acr_timing::stage_overall_markers::load_for_pacenote_geojson(p, cache).cloned());
    state.overall_finish_recorded = false;
}

fn attach_stage_timing_sectors(
    state: &mut LiveTimingState,
    stage_slug: &str,
    sectors_dir: &Path,
    cache: &mut acr_timing::timing_sectors::SectorCache,
) {
    state.stage_sector_session = acr_timing::timing_sectors::load_for_stage_slug(
        stage_slug,
        sectors_dir,
        cache,
    )
    .map(|m| acr_timing::stage_sector_timing::StageSectorSession::new(m.clone()));
}

/// Attach calibrated stage sectors when `[timing.ref_stage_sectors]` maps the locked reference track.
fn ensure_stage_timing_sectors(
    state: &mut LiveTimingState,
    reference_track: &str,
    stage_timing: &acr_timing::stage_timing_config::StageTimingConfig,
    cache: &mut acr_timing::timing_sectors::SectorCache,
    active_stage_slug: &mut Option<String>,
    pos: Option<(f64, f64)>,
) {
    if state.stage_sector_session.is_some() {
        return;
    }
    let Some(slug) = stage_timing.stage_slug_for_reference(reference_track) else {
        return;
    };
    attach_stage_timing_sectors(state, &slug, &stage_timing.sectors_dir(), cache);
    if let Some(sess) = state.stage_sector_session.as_ref() {
        eprintln!(
            "stage timing sectors: {} ({} markers, {} timed legs) [timing config, ref {}]",
            sess.markers.stage_slug,
            sess.markers.markers.len(),
            sess.markers.sector_leg_count,
            reference_track
        );
        if let Some(start) = sess.markers.markers.first() {
            eprintln!(
                "  timing start @ ({:.0}, {:.0}) — drive through calibrated markers in order (radius {:.0} m)",
                start.x,
                start.z,
                stage_timing.stage_sector_radius_m()
            );
            if let Some((x, z)) = pos {
                let d = acr_timing::timing_sectors::dist_xz(x, z, start.x, start.z);
                eprintln!(
                    "  you are {:.0}m from timing start (stage splits are not the ring SHP sectors)",
                    d
                );
            }
        }
        *active_stage_slug = Some(slug);
    }
}

fn flush_stage_sector_run(
    state: &mut LiveTimingState,
    cfg: &CliConfig,
    car_model: &str,
    run_counter: &mut usize,
) {
    let Some(session) = state.stage_sector_session.as_mut() else {
        return;
    };
    if !session.run.any_sector() {
        session.run.reset_run();
        return;
    }
    match acr_timing::stage_sector_timing::flush_run_to_html(
        session,
        &cfg.stage_timing.html_dir(),
        car_model,
        run_counter,
    ) {
        Ok(Some(_path)) => {}
        Ok(None) => {}
        Err(e) => eprintln!("stage sector HTML write failed: {}", e),
    }
}

impl LiveTimingState {
    fn new(ring_ids: Vec<i32>) -> Self {
        Self {
            tracker: SectorPassTracker::new(ring_ids.len().max(1)),
            ring_ids,
            last_anchor_t_sec: None,
            last_anchor_instant: None,
            last_anchor_drive_m: None,
            last_sector_idx: None,
            start_stage_pos: None,
            start_stage_since: None,
            start_stage_last_report_sec: -1,
            start_armed: false,
            start_anchor_t_sec: None,
            start_anchor_instant: None,
            start_anchor_drive_m: None,
            cooldown_until: HashMap::new(),
            overall_markers: None,
            overall_finish_recorded: false,
            stage_sector_session: None,
            leg_stats: SectorLegStatsAccumulator::default(),
            leg_entry_speed_kmh: None,
        }
    }

}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    ctrlc_handler();
    let cfg = parse_args(std::env::args().collect())?;
    let ref_files = resolve_reference_files(&cfg.refs)?;
    let labels = load_labels(&cfg)?;
    let refs = load_references(
        &ref_files,
        cfg.downsample,
        cfg.min_ref_spacing_m,
        &labels,
    )?;
    if refs.is_empty() {
        return Err("No valid references loaded".into());
    }

    if cfg.live {
        run_live(&refs, &cfg)?;
        #[cfg(windows)]
        {
            if cfg.rtss {
                let _ = acr_timing::rtss_osd::release(&cfg.rtss_owner);
            }
        }
    } else {
        let input = cfg
            .input
            .as_ref()
            .ok_or("Need --input <file.rkyv> unless --live is set")?;
        run_offline(&refs, input, &cfg)?;
    }
    Ok(())
}

#[derive(Debug)]
struct CliConfig {
    refs: Vec<PathBuf>,
    input: Option<PathBuf>,
    live: bool,
    downsample: usize,
    coarse_buffer_m: f64,
    coarse_required_ratio: f64,
    history_points: usize,
    live_rate_hz: u64,
    min_ref_spacing_m: f64,
    labels_path: Option<PathBuf>,
    overlay_file: PathBuf,
    rtss: bool,
    rtss_owner: String,
    rtss_slot: u32,
    rtss_clear_all: bool,
    sectors_shp: Option<PathBuf>,
    sectors_coord_space: SectorsCoordSpace,
    sector_track_field: String,
    sector_id_field: String,
    timing_db_path: PathBuf,
    sector_cross_cooldown_ms: u64,
    sector_search_radius_m: f64,
    track_keep_max_dist_m: f64,
    track_switch_min_gain: f64,
    track_lock_after_sec: f64,
    #[allow(dead_code)]
    track_unlock_speed_kmh: f64,
    #[allow(dead_code)]
    track_unlock_hold_sec: f64,
    start_points_geojson: PathBuf,
    start_prefilter_radius_m: f64,
    /// With a non-empty start_points index: max speed (km/h) to treat as standstill for grid pick.
    grid_standstill_max_speed_kmh: f64,
    /// Player must be within this radius (m) of some start anchor to open the track list.
    grid_start_trigger_radius_m: f64,
    /// Before `grid_start_wide_after_sec` standstill in the trigger zone: list tracks with a start in this radius.
    grid_start_list_radius_initial_m: f64,
    /// After this many seconds standstill in the trigger zone, expand the listing radius to `grid_start_list_radius_wide_m`.
    grid_start_wide_after_sec: f64,
    grid_start_list_radius_wide_m: f64,
    beep_on_split: bool,
    split_beep: SplitBeepConfig,
    pacenotes: Option<PacenoteConfig>,
    /// Calibrated stage-sector timing (independent of pacenotes).
    stage_timing: acr_timing::stage_timing_config::StageTimingConfig,
    /// Live: print `{:#?}` of the last received physics map at most once per second.
    debug_physics_1hz: bool,
}

fn parse_args(args: Vec<String>) -> Result<CliConfig, Box<dyn std::error::Error>> {
    let mut config_path: Option<PathBuf> = None;
    let mut timing_config_path: Option<PathBuf> = None;
    let mut pacenotes_config_path: Option<PathBuf> = None;
    let mut scan_i = 1;
    while scan_i < args.len() {
        match args[scan_i].as_str() {
            "--config" => {
                config_path = Some(PathBuf::from(
                    args.get(scan_i + 1).ok_or("--config needs a TOML path")?,
                ));
                scan_i += 1;
            }
            "--timing-config" => {
                timing_config_path = Some(PathBuf::from(
                    args.get(scan_i + 1).ok_or("--timing-config needs a TOML path")?,
                ));
                scan_i += 1;
            }
            "--pacenotes-config" => {
                pacenotes_config_path = Some(PathBuf::from(
                    args.get(scan_i + 1)
                        .ok_or("--pacenotes-config needs a TOML path")?,
                ));
                scan_i += 1;
            }
            _ => {}
        }
        scan_i += 1;
    }
    let loaded = app_config::load_all(
        config_path.as_deref(),
        timing_config_path.as_deref(),
        pacenotes_config_path.as_deref(),
    )?;
    log_loaded_configs(&loaded);

    let tm = &loaded.track_match;
    let timing = &loaded.timing;

    let mut refs: Vec<PathBuf> = tm
        .refs
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let mut input: Option<PathBuf> = tm.input.as_ref().map(PathBuf::from);
    let mut live = tm.live.unwrap_or(false);
    let mut downsample = tm.downsample.unwrap_or(10usize);
    let mut coarse_buffer_m = tm.buffer.unwrap_or(30.0f64);
    let mut coarse_required_ratio = tm.required_ratio.unwrap_or(0.5f64);
    let mut history_points = tm.history_points.unwrap_or(200usize);
    let mut live_rate_hz = tm.rate.unwrap_or(5u64);
    let mut min_ref_spacing_m = tm.min_ref_spacing.unwrap_or(2.0f64);
    let mut labels_path: Option<PathBuf> = tm.labels.as_ref().map(PathBuf::from);
    let mut overlay_file: Option<PathBuf> = tm.overlay_file.as_ref().map(PathBuf::from);
    let mut rtss = tm.rtss.unwrap_or(false);
    let mut rtss_owner = tm
        .rtss_owner
        .clone()
        .unwrap_or_else(|| "acr_track_match".to_string());
    let mut rtss_slot = tm.rtss_slot.unwrap_or(0u32);
    let mut rtss_clear_all = tm.rtss_clear_all.unwrap_or(false);
    let mut sectors_shp: Option<PathBuf> = timing.sectors_shp.as_ref().map(PathBuf::from);
    let mut sector_track_field = timing
        .sector_track_field
        .clone()
        .unwrap_or_else(|| "src_layer".to_string());
    let mut sector_id_field = timing
        .sector_id_field
        .clone()
        .unwrap_or_else(|| "seg_id".to_string());
    let mut sectors_coord_space = timing
        .sectors_coord_space
        .as_deref()
        .map(SectorsCoordSpace::parse)
        .transpose()?
        .unwrap_or(SectorsCoordSpace::File);
    let mut timing_db_path: Option<PathBuf> = timing.timing_db.as_ref().map(PathBuf::from);
    let mut sector_cross_cooldown_ms = timing.sector_cooldown_ms.unwrap_or(500u64);
    let mut sector_search_radius_m = timing.sector_radius.unwrap_or(25.0f64);
    let mut track_keep_max_dist_m = tm.track_keep_max_dist.unwrap_or(15.0f64);
    let mut track_switch_min_gain = tm.track_switch_min_gain.unwrap_or(0.8f64);
    let mut track_lock_after_sec = tm.track_lock_after_sec.unwrap_or(10.0f64);
    let mut track_unlock_speed_kmh = 3.0f64;
    let mut track_unlock_hold_sec = 5.0f64;
    let mut start_points_geojson = timing
        .start_points_geojson
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("timing/start_points.geojson"));
    let mut start_prefilter_radius_m = timing.start_prefilter_radius.unwrap_or(20.0f64);
    let grid_standstill_max_speed_kmh = timing
        .grid_standstill_max_speed_kmh
        .unwrap_or(DEFAULT_GRID_STANDSTILL_MAX_SPEED_KMH);
    let grid_start_trigger_radius_m = timing
        .grid_start_trigger_radius_m
        .unwrap_or(DEFAULT_GRID_START_TRIGGER_RADIUS_M);
    let grid_start_list_radius_initial_m = timing
        .grid_start_list_radius_initial_m
        .unwrap_or(DEFAULT_GRID_START_LIST_RADIUS_INITIAL_M);
    let grid_start_wide_after_sec = timing
        .grid_start_wide_after_sec
        .unwrap_or(DEFAULT_GRID_START_WIDE_AFTER_SEC);
    let grid_start_list_radius_wide_m = timing
        .grid_start_list_radius_wide_m
        .unwrap_or(DEFAULT_GRID_START_LIST_RADIUS_WIDE_M);
    let mut beep_on_split = timing.beep_on_split.unwrap_or(false);
    let split_beep = timing.beep.clone().unwrap_or_default();
    #[cfg(feature = "acr_timing_bin")]
    let pacenotes = None;
    #[cfg(not(feature = "acr_timing_bin"))]
    let pacenotes = loaded.pacenotes.clone();
    let mut debug_physics_1hz = tm.debug_physics_1hz.unwrap_or(false);
    let stage_timing = timing.stage_timing.clone();

    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--config" | "--timing-config" | "--pacenotes-config" => {
                i += 1;
            }
            "--refs" => {
                let next = args.get(i + 1).ok_or("--refs needs comma-separated paths")?;
                refs = next
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect();
                i += 1;
            }
            "--input" => {
                let next = args.get(i + 1).ok_or("--input needs a .rkyv path")?;
                input = Some(PathBuf::from(next));
                i += 1;
            }
            "--live" => live = true,
            "--downsample" => {
                let v = args
                    .get(i + 1)
                    .ok_or("--downsample needs integer")?
                    .parse::<usize>()?;
                if v == 0 {
                    return Err("--downsample must be >= 1".into());
                }
                downsample = v;
                i += 1;
            }
            "--buffer" => {
                coarse_buffer_m = args
                    .get(i + 1)
                    .ok_or("--buffer needs meters value")?
                    .parse::<f64>()?;
                i += 1;
            }
            "--required-ratio" => {
                coarse_required_ratio = args
                    .get(i + 1)
                    .ok_or("--required-ratio needs value between 0 and 1")?
                    .parse::<f64>()?;
                i += 1;
            }
            "--history-points" => {
                history_points = args
                    .get(i + 1)
                    .ok_or("--history-points needs integer")?
                    .parse::<usize>()?;
                i += 1;
            }
            "--rate" => {
                live_rate_hz = args
                    .get(i + 1)
                    .ok_or("--rate needs integer Hz")?
                    .parse::<u64>()?
                    .max(1);
                i += 1;
            }
            "--min-ref-spacing" => {
                min_ref_spacing_m = args
                    .get(i + 1)
                    .ok_or("--min-ref-spacing needs meters value")?
                    .parse::<f64>()?;
                i += 1;
            }
            "--labels" => {
                labels_path = Some(PathBuf::from(
                    args.get(i + 1).ok_or("--labels needs a TOML path")?,
                ));
                i += 1;
            }
            "--overlay-file" => {
                overlay_file = Some(PathBuf::from(
                    args.get(i + 1).ok_or("--overlay-file needs a path")?,
                ));
                i += 1;
            }
            "--rtss" => rtss = true,
            "--rtss-owner" => {
                rtss_owner = args
                    .get(i + 1)
                    .ok_or("--rtss-owner needs a string")?
                    .clone();
                i += 1;
            }
            "--rtss-slot" => {
                rtss_slot = args
                    .get(i + 1)
                    .ok_or("--rtss-slot needs integer")?
                    .parse::<u32>()?;
                i += 1;
            }
            "--rtss-clear-all" => rtss_clear_all = true,
            "--sectors-shp" => {
                sectors_shp = Some(PathBuf::from(
                    args.get(i + 1).ok_or("--sectors-shp needs .shp path")?,
                ));
                i += 1;
            }
            "--sector-track-field" => {
                sector_track_field = args
                    .get(i + 1)
                    .ok_or("--sector-track-field needs field name")?
                    .to_string();
                i += 1;
            }
            "--sector-id-field" => {
                sector_id_field = args
                    .get(i + 1)
                    .ok_or("--sector-id-field needs field name")?
                    .to_string();
                i += 1;
            }
            "--sectors-coord-space" => {
                sectors_coord_space = SectorsCoordSpace::parse(
                    args.get(i + 1)
                        .ok_or("--sectors-coord-space needs file|game")?,
                )?;
                i += 1;
            }
            "--timing-db" => {
                timing_db_path = Some(PathBuf::from(
                    args.get(i + 1).ok_or("--timing-db needs path")?,
                ));
                i += 1;
            }
            "--sector-cooldown-ms" => {
                sector_cross_cooldown_ms = args
                    .get(i + 1)
                    .ok_or("--sector-cooldown-ms needs integer")?
                    .parse::<u64>()?;
                i += 1;
            }
            "--sector-radius" => {
                sector_search_radius_m = args
                    .get(i + 1)
                    .ok_or("--sector-radius needs meters value")?
                    .parse::<f64>()?;
                i += 1;
            }
            "--track-keep-max-dist" => {
                track_keep_max_dist_m = args
                    .get(i + 1)
                    .ok_or("--track-keep-max-dist needs meters value")?
                    .parse::<f64>()?;
                i += 1;
            }
            "--track-switch-min-gain" => {
                track_switch_min_gain = args
                    .get(i + 1)
                    .ok_or("--track-switch-min-gain needs score delta")?
                    .parse::<f64>()?;
                i += 1;
            }
            "--track-lock-after-sec" => {
                track_lock_after_sec = args
                    .get(i + 1)
                    .ok_or("--track-lock-after-sec needs seconds value")?
                    .parse::<f64>()?;
                i += 1;
            }
            "--track-unlock-speed-kmh" => {
                track_unlock_speed_kmh = args
                    .get(i + 1)
                    .ok_or("--track-unlock-speed-kmh needs speed value")?
                    .parse::<f64>()?;
                i += 1;
            }
            "--track-unlock-hold-sec" => {
                track_unlock_hold_sec = args
                    .get(i + 1)
                    .ok_or("--track-unlock-hold-sec needs seconds value")?
                    .parse::<f64>()?;
                i += 1;
            }
            "--start-points-geojson" => {
                start_points_geojson = PathBuf::from(
                    args.get(i + 1).ok_or("--start-points-geojson needs path")?,
                );
                i += 1;
            }
            "--start-prefilter-radius" => {
                start_prefilter_radius_m = args
                    .get(i + 1)
                    .ok_or("--start-prefilter-radius needs meters value")?
                    .parse::<f64>()?;
                i += 1;
            }
            "--beep-on-split" => beep_on_split = true,
            "--debug-physics-1hz" => debug_physics_1hz = true,
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    if refs.is_empty() {
        return Err("Need --refs refA,refB,refC".into());
    }
    if !live && input.is_none() {
        return Err("Need --input <file.rkyv> for offline mode".into());
    }
    if !(0.0..=1.0).contains(&coarse_required_ratio) {
        return Err("--required-ratio must be between 0 and 1".into());
    }

    let overlay_file = overlay_file.unwrap_or_else(|| {
        let cfg = config::load_config();
        config::resolve_notes_dir(&cfg.recorder).join("acr_detected_track.txt")
    });
    let timing_db_path = timing_db_path.unwrap_or_else(|| {
        let cfg = config::load_config();
        config::resolve_notes_dir(&cfg.recorder).join("timing.db")
    });

    Ok(CliConfig {
        refs,
        input,
        live,
        downsample,
        coarse_buffer_m,
        coarse_required_ratio,
        history_points,
        live_rate_hz,
        min_ref_spacing_m,
        labels_path,
        overlay_file,
        rtss,
        rtss_owner,
        rtss_slot,
        rtss_clear_all,
        sectors_shp,
        sectors_coord_space,
        sector_track_field,
        sector_id_field,
        timing_db_path,
        sector_cross_cooldown_ms,
        sector_search_radius_m,
        track_keep_max_dist_m,
        track_switch_min_gain,
        track_lock_after_sec,
        track_unlock_speed_kmh,
        track_unlock_hold_sec,
        start_points_geojson,
        start_prefilter_radius_m,
        grid_standstill_max_speed_kmh,
        grid_start_trigger_radius_m,
        grid_start_list_radius_initial_m,
        grid_start_wide_after_sec,
        grid_start_list_radius_wide_m,
        beep_on_split,
        split_beep,
        pacenotes,
        stage_timing,
        debug_physics_1hz,
    })
}

fn print_usage() {
    #[cfg(feature = "acr_timing_bin")]
    let tool = "acr_timing";
    #[cfg(not(feature = "acr_timing_bin"))]
    let tool = "acr_track_match";
    eprintln!(
        "Usage: {} [--config acr_track_match.toml] --refs A.rkyv,B.points.shp,C.rkyv|reference_tracks [--input current.rkyv | --live]",
        tool
    );
    eprintln!("       --downsample N       Reference/query downsample step (default: 10)");
    eprintln!("       --buffer M           Coarse corridor radius in meters (default: 30)");
    eprintln!("       --required-ratio R   Coarse inlier ratio [0..1] (default: 0.5)");
    eprintln!("       --history-points N   Live history size (default: 200)");
    eprintln!("       --rate HZ            Live evaluation rate (default: 5)");
    eprintln!("       --min-ref-spacing M  Minimum spacing for loaded reference points (default: 2.0m)");
    eprintln!("       --labels FILE.toml   Optional labels mapping for reference files");
    eprintln!("       --overlay-file PATH  Write live detection message to file");
    eprintln!("       --rtss                 Also push message to RTSS OSD (Windows)");
    eprintln!("       --rtss-owner NAME      RTSS OSD owner id (default: acr_track_match)");
    eprintln!("       --rtss-slot N          Force RTSS slot N (0 = auto, default: 0)");
    eprintln!("       --rtss-clear-all       Clear all RTSS slots once at startup (careful: clears other OSD sources)");
    eprintln!("       --sectors-shp FILE.shp Optional sector boundaries LineString SHP (timing)");
    eprintln!("       --sectors-coord-space file|game  SHP vertex coords (default: file = GIS swap)");
    eprintln!("       --sector-track-field F Track field in sectors SHP (default: src_layer)");
    eprintln!("       --sector-id-field F    Sector id field in sectors SHP (default: seg_id)");
    eprintln!("       --timing-db PATH       Separate SQLite timing DB path (default: notes_dir/timing.db)");
    eprintln!("       --sector-cooldown-ms N Ignore re-trigger for same sector N ms (default: 500)");
    eprintln!("       --sector-radius M      Candidate search radius around player segment (default: 25m)");
    eprintln!("       --track-keep-max-dist M Keep current track while its mean_dist <= M (default: 15m)");
    eprintln!("       --track-switch-min-gain G Switch only if new score is better by >= G (default: 0.8)");
    eprintln!("       --track-lock-after-sec S Lock selected track after S seconds stable match (default: 10; no geometry unlock once locked)");
    eprintln!("       --track-unlock-speed-kmh V (legacy, unused) was low-speed unlock");
    eprintln!("       --track-unlock-hold-sec T (legacy, unused) was low-speed unlock hold");
    eprintln!("       --start-points-geojson FILE Save detected start anchors as GeoJSON points");
    eprintln!("       --start-prefilter-radius M Legacy when no start_points file: prefer unique track within M (default: 20)");
    eprintln!("Grid (when start_points.geojson has anchors): standstill ≤ grid_standstill_max_speed_kmh, within grid_start_trigger_radius_m of a start → pick list; after grid_start_wide_after_sec stillstand list expands to grid_start_list_radius_wide_m (see TOML keys).");
    eprintln!("       --beep-on-split        Play split sound via default audio (see [beep] in TOML)");
    eprintln!("       --debug-physics-1hz    Live: stderr dump of last PhysicsMap (~1/s, Rust pretty-Debug)");
    eprintln!("       --config FILE.toml          Track-match config (default: acr_track_match.toml)");
    eprintln!("       --timing-config FILE.toml   Timing config (default: acr_timing.toml, same dir as --config)");
    eprintln!("       --pacenotes-config FILE.toml Pacenotes config (default: acr_pacenotes.toml, same dir)");
}

fn log_loaded_configs(loaded: &app_config::LoadedAppConfig) {
    match &loaded.track_match_path {
        Some(p) => eprintln!("acr_track_match: loaded {}", p.display()),
        None => eprintln!(
            "acr_track_match: no {} (defaults only)",
            app_config::TRACK_MATCH_CONFIG_FILE
        ),
    }
    match &loaded.timing_path {
        Some(p) => eprintln!("acr_timing: loaded {}", p.display()),
        None => eprintln!(
            "acr_timing: no {} (timing defaults only)",
            acr_timing::timing_config_file::TIMING_CONFIG_FILE
        ),
    }
    match &loaded.pacenotes_path {
        Some(p) => eprintln!("acr_pacenotes: loaded {}", p.display()),
        None => eprintln!(
            "acr_pacenotes: no {} (pacenotes off unless enabled via CLI)",
            app_config::PACENOTES_CONFIG_FILE
        ),
    }
}

#[derive(Debug, Deserialize, Default)]
struct TrackLabelsFile {
    #[serde(default)]
    labels: std::collections::HashMap<String, String>,
}

fn resolve_reference_files(ref_inputs: &[PathBuf]) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for p in ref_inputs {
        if p.is_dir() {
            for entry in std::fs::read_dir(p)? {
                let path = entry?.path();
                if path
                    .extension()
                    .map(|e| e == "rkyv" || e == "shp")
                    .unwrap_or(false)
                {
                    out.push(path);
                }
            }
        } else {
            out.push(p.clone());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn load_references(
    ref_paths: &[PathBuf],
    downsample: usize,
    min_ref_spacing_m: f64,
    labels: &std::collections::HashMap<String, String>,
) -> Result<Vec<ReferenceTrack>, Box<dyn std::error::Error>> {
    let mut refs = Vec::new();
    for p in ref_paths {
        let loaded = if p.extension().map(|e| e == "shp").unwrap_or(false) {
            load_points_from_shp(p)?
        } else if p.extension().map(|e| e == "rkyv").unwrap_or(false) {
            load_points_from_rkyv(p, downsample)?
        } else {
            return Err(format!("Unsupported reference file: {}", p.display()).into());
        };
        let loaded = thin_points_by_spacing(&loaded, min_ref_spacing_m);
        if loaded.len() < 5 {
            return Err(format!("Reference too short: {}", p.display()).into());
        }
        refs.push(ReferenceTrack {
            name: labels
                .get(
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown"),
                )
                .cloned()
                .unwrap_or_else(|| p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()),
            headings: compute_headings(&loaded),
            points: loaded,
        });
    }
    Ok(refs)
}

fn thin_points_by_spacing(points: &[Point2], min_spacing_m: f64) -> Vec<Point2> {
    if points.is_empty() || min_spacing_m <= 0.0 {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity(points.len());
    let mut last = points[0];
    out.push(last);
    for &p in points.iter().skip(1) {
        if dist(last, p) >= min_spacing_m {
            out.push(p);
            last = p;
        }
    }
    if let Some(&tail) = points.last() {
        if out.last().map_or(true, |v| dist(*v, tail) > 0.1) {
            out.push(tail);
        }
    }
    out
}

fn load_points_from_rkyv(
    path: &Path,
    downsample: usize,
) -> Result<Vec<Point2>, Box<dyn std::error::Error>> {
    let graphics_path = path.with_extension("graphics.rkyv");
    let (_, g) = rkyv_reader::read_graphics_rkyv(&graphics_path)?;
    let pts = g
        .iter()
        .enumerate()
        .step_by(downsample)
        .map(|(_, r)| Point2 {
            x: r.car_coordinates_x as f64,
            z: r.car_coordinates_z as f64,
        })
        .collect();
    Ok(pts)
}

fn load_points_from_shp(path: &Path) -> Result<Vec<Point2>, Box<dyn std::error::Error>> {
    let mut reader = shapefile::Reader::from_path(path)?;
    let mut pts = Vec::new();
    for item in reader.iter_shapes_and_records() {
        let (shape, _) = item?;
        if let shapefile::Shape::Point(p) = shape {
            let (x, z) = acr_telemetry::gis::file_to_game_xz(p.x, p.y);
            pts.push(Point2 { x, z });
        }
    }
    Ok(pts)
}

fn compute_headings(points: &[Point2]) -> Vec<f64> {
    let mut out = vec![0.0; points.len()];
    if points.len() < 2 {
        return out;
    }
    for i in 0..points.len() - 1 {
        out[i] = (points[i + 1].z - points[i].z).atan2(points[i + 1].x - points[i].x);
    }
    out[points.len() - 1] = out[points.len() - 2];
    out
}

/// Build a query path for `match_tracks` even when the car has barely moved: pad with the
/// current position until `min_len` samples so coarse matching and pacenote hints still run.
fn live_match_query(history: &VecDeque<Point2>, p: Point2, min_len: usize) -> Vec<Point2> {
    let mut q: Vec<Point2> = history.iter().copied().collect();
    if q.is_empty() {
        q.push(p);
    }
    while q.len() < min_len {
        q.push(p);
    }
    q
}

fn run_offline(
    refs: &[ReferenceTrack],
    input: &Path,
    cfg: &CliConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let query = load_points_from_rkyv(input, cfg.downsample)?;
    let scores = match_tracks(&query, refs, cfg);
    print_scores(&scores);
    Ok(())
}

/// Narrow start-point candidates using ACC `statics.track_spline_length` when catalogued.
fn filter_tracks_by_spline_length(
    observed_m: f32,
    stems: Vec<String>,
    catalog: &HashMap<String, f32>,
) -> Vec<String> {
    if observed_m <= 1.0 || catalog.is_empty() {
        return stems;
    }
    let narrowed: Vec<String> = acr_timing::track_spline_ref::matching_stems(
        observed_m,
        stems.iter().map(String::as_str),
        catalog,
    )
    .into_iter()
    .map(str::to_string)
    .collect();
    if narrowed.is_empty() {
        stems
    } else {
        narrowed
    }
}

/// Track + timing when locking at the grid from `start_points.geojson` or pacenote UI.
/// Pacenote path: caller sets via catalog or `apply_pacenote_first_anchor_resolution`.
fn activate_standstill_track_lock(
    track_name: &str,
    car_model_now: &str,
    p: Point2,
    refs: &[ReferenceTrack],
    sector_sets: &HashMap<String, SectorSet>,
    timing_conn: &rusqlite::Connection,
    stage_timing: &acr_timing::stage_timing_config::StageTimingConfig,
    timing_sector_cache: &mut acr_timing::timing_sectors::SectorCache,
    active_timing_stage_slug: &mut Option<String>,
    locked_track: &mut Option<String>,
    locked_car_model: &mut Option<String>,
    active_track_name: &mut Option<String>,
    stable_selected: &mut Option<(String, Instant)>,
    timing_state: &mut Option<LiveTimingState>,
    sector_status_line: &mut Option<(String, Instant)>,
    detected_track_line: &mut Option<(String, Instant)>,
    last_sector_wait_log: &mut Instant,
    history: &mut VecDeque<Point2>,
    last_pt: &mut Option<Point2>,
    total_drive_m: &mut f64,
    log_line: &str,
) {
    if !refs.iter().any(|r| r.name == track_name) {
        return;
    }
    *locked_track = Some(track_name.to_string());
    *locked_car_model = if car_model_now.is_empty() {
        None
    } else {
        Some(car_model_now.to_string())
    };
    *active_track_name = Some(track_name.to_string());
    *stable_selected = Some((track_name.to_string(), Instant::now()));
    *timing_state = if let Some(s) = sector_sets.get(track_name) {
        let line = "waiting for sector passing...".to_string();
        eprintln!("{} ({})", line, track_name);
        *sector_status_line = Some((line, Instant::now()));
        *detected_track_line = Some((
            format!("detected track {}", track_name),
            Instant::now(),
        ));
        Some(LiveTimingState::new(s.ring_ids.clone()))
    } else {
        let line = "no sector set for detected track".to_string();
        eprintln!("{} ({})", line, track_name);
        *sector_status_line = Some((line, Instant::now()));
        *detected_track_line = Some((
            format!("detected track {}", track_name),
            Instant::now(),
        ));
        None
    };
    *last_sector_wait_log = Instant::now();
    reset_live_route_samples(history, last_pt, total_drive_m);
    if let (Some(state), Some(set)) = (timing_state.as_mut(), sector_sets.get(track_name)) {
        seed_sector_tracker_at_position(state, set, p);
        ensure_stage_timing_sectors(
            state,
            track_name,
            stage_timing,
            timing_sector_cache,
            active_timing_stage_slug,
            Some((p.x, p.z)),
        );
    }
    eprintln!("{}", log_line);
    if let Ok(n) = acr_timing::timing_db::promote_pending_for_track(timing_conn, track_name) {
        if n > 0 {
            eprintln!("promoted {} pending split(s) for {}", n, track_name);
        }
    }
}

fn run_live(refs: &[ReferenceTrack], cfg: &CliConfig) -> Result<(), Box<dyn std::error::Error>> {
    let mut acc = ACCSharedMemory::new()?;
    let timing_conn = acr_timing::timing_db::open_or_create(&cfg.timing_db_path)?;
    let spline_catalog = acr_timing::track_spline_ref::load_catalog(Path::new(
        "timing/track_spline_lengths.toml",
    ))?;
    if !spline_catalog.is_empty() {
        eprintln!(
            "track_spline_lengths: {} reference stem(s) in catalog",
            spline_catalog.len()
        );
    }
    let start_index = load_start_points_index(&cfg.start_points_geojson)?;
    let sector_sets = if let Some(sectors_path) = &cfg.sectors_shp {
        eprintln!(
            "sectors SHP: {} (coord_space={:?})",
            sectors_path.display(),
            cfg.sectors_coord_space
        );
        load_sector_sets_from_shp(
            sectors_path,
            cfg.sectors_coord_space,
            &cfg.sector_track_field,
            &cfg.sector_id_field,
            refs,
        )?
    } else {
        HashMap::new()
    };
    let pacenote_cfg = cfg
        .pacenotes
        .clone()
        .and_then(|p| if p.enabled { Some(p) } else { None });
    if let Some(pacenote_cfg) = &pacenote_cfg {
        if pacenote_cfg.voice_dir.is_none() {
            eprintln!("pacenotes enabled but voice_dir is missing; voice playback disabled");
        }
        if pacenote_cfg.geojson.is_none() && pacenote_cfg.pacenotes_dir.is_none() {
            eprintln!("pacenotes enabled but neither geojson nor pacenotes_dir is set");
        }
    }
    let pacenote_player = pacenote_cfg
        .as_ref()
        .and_then(|p| p.voice_dir.as_ref())
        .map(|dir| PacenoteVoicePlayer::spawn(dir.clone(), pacenote_cfg.as_ref().unwrap().volume));
    let pacenote_stage_catalog = pacenote_cfg
        .as_ref()
        .and_then(|p| p.pacenotes_dir.as_deref())
        .and_then(|dir| match PacenoteStageCatalog::load_dir(dir) {
            Ok(catalog) => {
                eprintln!(
                    "pacenote start catalog: {} stage(s) from {}",
                    catalog.len(),
                    dir.display()
                );
                Some(catalog)
            }
            Err(e) => {
                eprintln!("pacenote start catalog failed ({}): {}", dir.display(), e);
                None
            }
        });
    let ref_names_for_pacenotes: HashSet<String> = refs.iter().map(|r| r.name.clone()).collect();
    let mut history: VecDeque<Point2> = VecDeque::with_capacity(cfg.history_points + 10);
    let eval_interval = Duration::from_millis((1000 / cfg.live_rate_hz.max(1)) as u64);
    let mut last_eval = Instant::now();
    let mut last_no_data_log = Instant::now();
    let mut have_physics_frame = false; // first successful new physics read from shared memory
    let mut last_physics_debug_at = Instant::now() - Duration::from_secs(1);
    let mut last_pt: Option<Point2> = None;
    let mut total_drive_m = 0.0f64;
    let mut timing_state: Option<LiveTimingState> = None;
    let mut active_track_name: Option<String> = None;
    let mut latest_timing_line: Option<(String, Instant)> = None;
    let mut sector_status_line: Option<(String, Instant)> = None;
    let mut detected_track_line: Option<(String, Instant)> = None;
    let mut stable_selected: Option<(String, Instant)> = None;
    let mut locked_track: Option<String> = None;
    let mut locked_car_model: Option<String> = None;
    let mut pacenote_course: Option<PacenoteCourse> = None;
    let mut pacenote_course_track: Option<String> = None;
    let mut active_pacenote_stage_path: Option<PathBuf> = None;
    // Last seen `modified` time of the loaded pacenote GeoJSON (reload when file changes on disk).
    let mut pacenote_loaded_src_mtime: Option<SystemTime> = None;
    let mut triggered_pacenotes: HashSet<usize> = HashSet::new();
    let mut pacenote_ambiguous_pick: Option<AmbiguousPacenoteOverlayState> = None;
    let mut start_track_ambiguous_pick: Option<TrackStartPickOverlayState> = None;
    let mut grid_standstill_since: Option<Instant> = None;
    let mut teleport_unlock_pending_jump_m: Option<f64> = None;
    let mut teleport_unlock_stillstand_since: Option<Instant> = None;
    let mut teleport_unlock_driving_since: Option<Instant> = None;
    let start_points_mode = !start_index.is_empty();
    let mut last_pacenote_gear_eval = Instant::now();
    let mut pacenote_gear_extra_lead_sec = 0.0f64;
    let mut no_data_since: Option<Instant> = None;
    let mut last_sector_wait_log = Instant::now();
    let mut last_pacenote_anchor_help =
        Instant::now() - Duration::from_secs(PACENOTE_ANCHOR_HELP_SECS);
    // Limits repeat of "where do we go" voice + stderr when the ambiguous menu flickers open/closed.
    let mut last_pacenote_ambiguous_where_voice_at: Option<Instant> = None;
    let mut last_pacenote_ambiguous_help_log_at: Option<Instant> = None;
    let mut overall_marker_cache: acr_timing::stage_overall_markers::MarkerCache =
        HashMap::new();
    let mut timing_sector_cache: acr_timing::timing_sectors::SectorCache = HashMap::new();
    let mut stage_sector_html_run_counter: usize = 0;
    let mut active_timing_stage_slug: Option<String> = None;
    let mut stillstand_log_state = acr_timing::physics_wheel::StillstandLogState::default();
    let _ = std::fs::create_dir_all(&cfg.stage_timing.html_dir());
    // After Ctrl+Enter on the first-anchor picker, suppress reopening the menu while the same
    // anchors stay within radius (otherwise the overlay immediately reappears and Enter retriggers).
    let mut pacenote_manual_anchor_slug: Option<String> = None;
    let mut last_overlay_msg: String = compose_two_line_osd("detecting track...", "");
    let mut last_overlay_push = Instant::now();
    let overlay_dir = cfg
        .overlay_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&overlay_dir);
    #[cfg(windows)]
    {
        if cfg.rtss {
            // Always release our own owner on startup to avoid stale slot artifacts from prior runs.
            let _ = acr_timing::rtss_osd::release(&cfg.rtss_owner);
            if cfg.rtss_clear_all {
                match acr_timing::rtss_osd::clear_all() {
                    Ok(()) => eprintln!("RTSS cleanup: cleared all OSD slots."),
                    Err(e) => eprintln!("RTSS cleanup failed: {}", e),
                }
            }
        }
    }
    push_live_overlay(cfg, &last_overlay_msg, 2)?;
    eprintln!("live mode started; waiting for ACC shared memory...");

    while RUNNING.load(Ordering::Relaxed) {
        if let Some(data) = acc.read_shared_memory()? {
            no_data_since = None;
            if !have_physics_frame {
                have_physics_frame = true;
                eprintln!(
                    "ACC telemetry active (first physics packet_id={})",
                    data.physics.packet_id
                );
            }
            if cfg.debug_physics_1hz && last_physics_debug_at.elapsed() >= Duration::from_secs(1) {
                last_physics_debug_at = Instant::now();
                eprintln!(
                    "acr_track_match debug-physics-1hz (last new frame, packet_id={}):\n{:#?}\n---",
                    data.physics.packet_id, data.physics
                );
            }
            let car_model_now = data.statics.car_model.trim().to_string();
            let speed_kmh_now = data.physics.speed_kmh as f64;
            if let Some(lock_car) = &locked_car_model {
                if !car_model_now.is_empty() && car_model_now != *lock_car {
                    eprintln!(
                        "unlocking track lock due to car change: '{}' -> '{}'",
                        lock_car, car_model_now
                    );
                    locked_track = None;
                    locked_car_model = None;
                    stable_selected = None;
                    pacenote_ambiguous_pick = None;
                    start_track_ambiguous_pick = None;
                    grid_standstill_since = None;
                    teleport_unlock_pending_jump_m = None;
                    teleport_unlock_stillstand_since = None;
                    teleport_unlock_driving_since = None;
                    clear_pacenote_live(
                        &mut pacenote_course,
                        &mut pacenote_course_track,
                        &mut active_pacenote_stage_path,
                        &mut triggered_pacenotes,
                        &mut last_pacenote_gear_eval,
                        &mut pacenote_gear_extra_lead_sec,
                        &mut pacenote_manual_anchor_slug,
                        &mut pacenote_loaded_src_mtime,
                        true,
                    );
                }
            }
            let observed_spline_m = data.statics.track_spline_length;
            let default_coords = acc_shared_memory_rs::datatypes::Vector3f::new(0.0, 0.0, 0.0);
            let player_coords = data
                .graphics
                .car_coordinates
                .iter()
                .zip(&data.graphics.car_id)
                .find(|&(_, &id)| id == data.graphics.player_car_id)
                .map(|(coords, _)| coords)
                .unwrap_or(&default_coords);
            let p = Point2 {
                x: player_coords.x as f64,
                z: player_coords.z as f64,
            };
            let (stage_armed, stage_leg_elapsed, stage_next_label) = timing_state
                .as_ref()
                .and_then(|ts| ts.stage_sector_session.as_ref())
                .map(|sess| {
                    let now = Instant::now();
                    let next = sess
                        .markers
                        .markers
                        .get(sess.run.next_marker_idx)
                        .map(|m| m.label.as_str());
                    (
                        sess.run.armed,
                        sess.run.live_leg_elapsed_sec(now),
                        next,
                    )
                })
                .unwrap_or((false, None, None));
            acr_timing::physics_wheel::maybe_log_stillstand_position(
                &cfg.stage_timing,
                &mut stillstand_log_state,
                &data.physics,
                speed_kmh_now,
                acr_timing::physics_wheel::StillstandLogContext {
                    graphics_x: p.x,
                    graphics_z: p.z,
                    graphics_clock: data.graphics.clock as f64,
                    distance_traveled_m: data.graphics.distance_traveled as f64,
                    stage_armed,
                    stage_leg_elapsed_sec: stage_leg_elapsed,
                    stage_next_label,
                },
            );
            if locked_track.is_some() {
                if let Some(lp) = last_pt {
                    let jump_m = dist(lp, p);
                    if jump_m > START_LAYOUT_TELEPORT_RESET_M {
                        let record = teleport_unlock_pending_jump_m
                            .map_or(true, |prev| jump_m > prev);
                        if record {
                            teleport_unlock_pending_jump_m = Some(jump_m);
                            teleport_unlock_stillstand_since = None;
                            teleport_unlock_driving_since = None;
                            eprintln!(
                                "position jump {:.1} m (> {:.0} m): unlock after {:.0}s stillstand at start grid",
                                jump_m, START_LAYOUT_TELEPORT_RESET_M, TELEPORT_UNLOCK_STILL_SEC
                            );
                        }
                    }
                }
            }
            if let Some(jump_m) = teleport_unlock_pending_jump_m {
                let near_start = start_points_mode
                    && !tracks_within_start_points(
                        &start_index,
                        p,
                        cfg.grid_start_list_radius_wide_m,
                        refs,
                    )
                    .is_empty();
                if speed_kmh_now <= cfg.grid_standstill_max_speed_kmh {
                    teleport_unlock_driving_since = None;
                    if near_start {
                        if teleport_unlock_stillstand_since.is_none() {
                            teleport_unlock_stillstand_since = Some(Instant::now());
                        } else if teleport_unlock_stillstand_since
                            .unwrap()
                            .elapsed()
                            .as_secs_f64()
                            >= TELEPORT_UNLOCK_STILL_SEC
                        {
                            eprintln!(
                                "unlocking track lock: {:.1} m jump + {:.0}s stillstand at start grid",
                                jump_m, TELEPORT_UNLOCK_STILL_SEC
                            );
                            teleport_unlock_pending_jump_m = None;
                            teleport_unlock_stillstand_since = None;
                            let prev_locked = locked_track.clone();
                            if let Some(state) = timing_state.as_mut() {
                                let car_model =
                                    locked_car_model.as_deref().unwrap_or("unknown_car");
                                flush_stage_sector_run(
                                    state,
                                    &cfg,
                                    car_model,
                                    &mut stage_sector_html_run_counter,
                                );
                            }
                            locked_track = None;
                            locked_car_model = None;
                            stable_selected = None;
                            active_track_name = None;
                            timing_state = None;
                            active_timing_stage_slug = None;
                            latest_timing_line = None;
                            sector_status_line = Some((
                                format!(
                                    "reset after grid stillstand ({:.0}s, jump {:.0} m)",
                                    TELEPORT_UNLOCK_STILL_SEC, jump_m
                                ),
                                Instant::now(),
                            ));
                            detected_track_line = None;
                            history.clear();
                            last_pt = None;
                            total_drive_m = 0.0;
                            pacenote_ambiguous_pick = None;
                            start_track_ambiguous_pick = None;
                            grid_standstill_since = None;
                            clear_pacenote_live(
                                &mut pacenote_course,
                                &mut pacenote_course_track,
                                &mut active_pacenote_stage_path,
                                &mut triggered_pacenotes,
                                &mut last_pacenote_gear_eval,
                                &mut pacenote_gear_extra_lead_sec,
                                &mut pacenote_manual_anchor_slug,
                                &mut pacenote_loaded_src_mtime,
                                true,
                            );
                            let status = if let Some(name) = prev_locked.as_deref() {
                                format!("track reset {}", name)
                            } else {
                                "track reset".to_string()
                            };
                            let detail = format!("grid reset after jump {:.0} m", jump_m);
                            let msg = compose_two_line_osd(&status, &detail);
                            let _ = push_live_overlay(cfg, &msg, 2);
                            last_overlay_msg = msg;
                            last_overlay_push = Instant::now();
                        }
                    } else {
                        teleport_unlock_stillstand_since = None;
                    }
                } else {
                    teleport_unlock_stillstand_since = None;
                    if teleport_unlock_driving_since.is_none() {
                        teleport_unlock_driving_since = Some(Instant::now());
                    } else if teleport_unlock_driving_since
                        .unwrap()
                        .elapsed()
                        .as_secs_f64()
                        >= TELEPORT_PENDING_CLEAR_DRIVE_SEC
                    {
                        eprintln!(
                            "position jump pending cleared ({:.0}s driving, no start grid)",
                            TELEPORT_PENDING_CLEAR_DRIVE_SEC
                        );
                        teleport_unlock_pending_jump_m = None;
                        teleport_unlock_driving_since = None;
                    }
                }
            }
            let in_start_grid_trigger = start_points_mode
                && locked_track.is_none()
                && speed_kmh_now <= cfg.grid_standstill_max_speed_kmh
                && !tracks_within_start_points(
                    &start_index,
                    p,
                    cfg.grid_start_trigger_radius_m,
                    refs,
                )
                .is_empty();

            if in_start_grid_trigger {
                if grid_standstill_since.is_none() {
                    grid_standstill_since = Some(Instant::now());
                }
            } else {
                grid_standstill_since = None;
            }

            if locked_track.is_none() && start_points_mode {
                if !in_start_grid_trigger {
                    start_track_ambiguous_pick = None;
                } else {
                    let list_radius_m = if grid_standstill_since
                        .map(|t| t.elapsed().as_secs_f64() >= cfg.grid_start_wide_after_sec)
                        .unwrap_or(false)
                    {
                        cfg.grid_start_list_radius_wide_m
                    } else {
                        cfg.grid_start_list_radius_initial_m
                    };
                    let tracks_near = filter_tracks_by_spline_length(
                        observed_spline_m,
                        tracks_within_start_points(
                            &start_index,
                            p,
                            list_radius_m,
                            refs,
                        ),
                        &spline_catalog,
                    );
                    if let Some(ref mut st_ui) = start_track_ambiguous_pick {
                        if tracks_near.is_empty() {
                            start_track_ambiguous_pick = None;
                        } else {
                            let prev_name = st_ui.track_names.get(st_ui.index).cloned();
                            st_ui.track_names.clone_from(&tracks_near);
                            st_ui.index = prev_name
                                .and_then(|n| st_ui.track_names.iter().position(|t| t == &n))
                                .unwrap_or(0)
                                .min(st_ui.track_names.len().saturating_sub(1));
                            match st_ui.keys.poll() {
                                Some(PacenotePickerNav::Prev) => {
                                    let n = st_ui.track_names.len().max(1);
                                    st_ui.index = (st_ui.index + n - 1) % n;
                                }
                                Some(PacenotePickerNav::Next) => {
                                    let n = st_ui.track_names.len().max(1);
                                    st_ui.index = (st_ui.index + 1) % n;
                                }
                                Some(PacenotePickerNav::Confirm) => {
                                    if let Some(chosen) = st_ui.track_names.get(st_ui.index).cloned()
                                    {
                                        start_track_ambiguous_pick = None;
                                        pacenote_ambiguous_pick = None;
                                        pacenote_manual_anchor_slug = None;
                                        if refs.iter().any(|r| r.name == chosen) {
                                            activate_standstill_track_lock(
                                                &chosen,
                                                &car_model_now,
                                                p,
                                                refs,
                                                &sector_sets,
                                                &timing_conn,
                                                &cfg.stage_timing,
                                                &mut timing_sector_cache,
                                                &mut active_timing_stage_slug,
                                                &mut locked_track,
                                                &mut locked_car_model,
                                                &mut active_track_name,
                                                &mut stable_selected,
                                                &mut timing_state,
                                                &mut sector_status_line,
                                                &mut detected_track_line,
                                                &mut last_sector_wait_log,
                                                &mut history,
                                                &mut last_pt,
                                                &mut total_drive_m,
                                                &format!(
                                                    "track locked from start_points picker (list r={:.0} m, spline {:.1} m): {}",
                                                    list_radius_m, observed_spline_m, chosen
                                                ),
                                            );
                                            let pick_radius_m = cfg
                                                .grid_start_list_radius_wide_m
                                                .max(cfg.start_prefilter_radius_m);
                                            if let Some(catalog) = pacenote_stage_catalog.as_ref() {
                                                if let Some(pick) = catalog.select_from_position(
                                                    p.x,
                                                    p.z,
                                                    pick_radius_m,
                                                ) {
                                                    if pick.reference_track == chosen {
                                                        active_pacenote_stage_path =
                                                            Some(pick.path.clone());
                                                    }
                                                }
                                            }
                                            eprintln!("start_points: confirmed '{}'", chosen);
                                        }
                                    }
                                }
                                None => {}
                            }
                        }
                    } else if tracks_near.len() == 1 {
                        let chosen = tracks_near[0].clone();
                        pacenote_ambiguous_pick = None;
                        pacenote_manual_anchor_slug = None;
                        start_track_ambiguous_pick = None;
                        activate_standstill_track_lock(
                            &chosen,
                            &car_model_now,
                            p,
                            refs,
                            &sector_sets,
                            &timing_conn,
                            &cfg.stage_timing,
                            &mut timing_sector_cache,
                            &mut active_timing_stage_slug,
                            &mut locked_track,
                            &mut locked_car_model,
                            &mut active_track_name,
                            &mut stable_selected,
                            &mut timing_state,
                            &mut sector_status_line,
                            &mut detected_track_line,
                            &mut last_sector_wait_log,
                            &mut history,
                            &mut last_pt,
                            &mut total_drive_m,
                            &format!(
                                "track locked from start_points (unique + spline {:.1} m, r={:.0} m): {}",
                                observed_spline_m, list_radius_m, chosen
                            ),
                        );
                        let pick_radius_m = cfg
                            .grid_start_list_radius_wide_m
                            .max(cfg.start_prefilter_radius_m);
                        if let Some(catalog) = pacenote_stage_catalog.as_ref() {
                            if let Some(pick) =
                                catalog.select_from_position(p.x, p.z, pick_radius_m)
                            {
                                if pick.reference_track == chosen {
                                    active_pacenote_stage_path = Some(pick.path.clone());
                                }
                            }
                        }
                    } else if !tracks_near.is_empty() {
                        pacenote_ambiguous_pick = None;
                        pacenote_manual_anchor_slug = None;
                        eprintln!(
                            "start_points: {} track(s) within {:.0} m (spline {:.1} m; standstill ≤ {:.1} km/h; list widens to {:.0} m after {:.0}s)",
                            tracks_near.len(),
                            list_radius_m,
                            observed_spline_m,
                            cfg.grid_standstill_max_speed_kmh,
                            cfg.grid_start_list_radius_wide_m,
                            cfg.grid_start_wide_after_sec
                        );
                        start_track_ambiguous_pick = Some(TrackStartPickOverlayState {
                            track_names: tracks_near,
                            index: 0,
                            keys: PacenotePickerKeyTracker::new(),
                        });
                    }
                }
            }

            if start_track_ambiguous_pick.is_none() {
            if let (Some(catalog), Some(pc)) =
                (pacenote_stage_catalog.as_ref(), pacenote_cfg.as_ref())
            {
                if speed_kmh_now > pc.first_anchor_pick_max_speed_kmh {
                    pacenote_manual_anchor_slug = None;
                }
                if speed_kmh_now <= pc.first_anchor_pick_max_speed_kmh {
                    let pace_ref = locked_track
                        .as_deref()
                        .or(active_track_name.as_deref());
                    let lock_r = pc.first_anchor_lock_radius_m.max(0.1);
                    let menu_r = pc.first_anchor_menu_radius_m.max(lock_r);
                    let candidates_prio = if pc.ref_geojson_candidates.is_empty() {
                        None
                    } else {
                        Some(&pc.ref_geojson_candidates)
                    };
                    let resolved_explicit_paths: Option<Vec<PathBuf>> = pace_ref
                        .and_then(|pr| {
                            pacenote_course::ref_geojson_candidate_paths_ref(
                                &pc.ref_geojson_candidates,
                                pr,
                            )
                            .map(|v| {
                                v.iter()
                                    .cloned()
                                    .filter(|p| p.is_file())
                                    .collect::<Vec<_>>()
                            })
                        })
                        .filter(|v| v.len() >= 2);
                    let explicit_multi = resolved_explicit_paths.is_some();
                    let (hits_lock, hits_menu) =
                        if let Some(ref paths) = resolved_explicit_paths {
                            (
                                catalog.first_anchor_hits_for_explicit_paths(
                                    paths, p.x, p.z, lock_r,
                                ),
                                catalog.first_anchor_hits_for_explicit_paths(
                                    paths, p.x, p.z, menu_r,
                                ),
                            )
                        } else {
                            (
                                catalog.first_anchor_candidates_within(
                                    p.x,
                                    p.z,
                                    lock_r,
                                    Some(&ref_names_for_pacenotes),
                                    pace_ref,
                                    candidates_prio,
                                ),
                                catalog.first_anchor_candidates_within(
                                    p.x,
                                    p.z,
                                    menu_r,
                                    Some(&ref_names_for_pacenotes),
                                    pace_ref,
                                    candidates_prio,
                                ),
                            )
                        };
                    if hits_menu.is_empty() {
                        pacenote_manual_anchor_slug = None;
                    }
                    if !explicit_multi && hits_lock.len() == 1 {
                        let pick0 = &hits_lock[0].1;
                        if locked_track.is_none() {
                            activate_standstill_track_lock(
                                pick0.reference_track.as_str(),
                                &car_model_now,
                                p,
                                refs,
                                &sector_sets,
                                &timing_conn,
                                &cfg.stage_timing,
                                &mut timing_sector_cache,
                                &mut active_timing_stage_slug,
                                &mut locked_track,
                                &mut locked_car_model,
                                &mut active_track_name,
                                &mut stable_selected,
                                &mut timing_state,
                                &mut sector_status_line,
                                &mut detected_track_line,
                                &mut last_sector_wait_log,
                                &mut history,
                                &mut last_pt,
                                &mut total_drive_m,
                                &format!(
                                    "track locked from pacenote first-anchor (unique, lock r={:.0} m): {}",
                                    lock_r, pick0.reference_track
                                ),
                            );
                        }
                        apply_pacenote_first_anchor_resolution(
                            pick0,
                            &mut active_pacenote_stage_path,
                            pc,
                            locked_track.as_deref(),
                            active_track_name.as_deref(),
                            &mut pacenote_course,
                            &mut pacenote_course_track,
                            &mut triggered_pacenotes,
                            &mut last_pacenote_gear_eval,
                            &mut pacenote_gear_extra_lead_sec,
                            &mut pacenote_manual_anchor_slug,
                            &mut pacenote_loaded_src_mtime,
                        );
                        pacenote_manual_anchor_slug = None;
                        pacenote_ambiguous_pick = None;
                        last_pacenote_ambiguous_where_voice_at = None;
                        last_pacenote_ambiguous_help_log_at = None;
                    } else {
                        if pacenote_ambiguous_pick.is_some() && hits_menu.is_empty() {
                            pacenote_ambiguous_pick = None;
                        }
                        if let Some(ref mut ui) = pacenote_ambiguous_pick {
                            if hits_menu.is_empty() {
                                pacenote_ambiguous_pick = None;
                            } else if !explicit_multi && hits_menu.len() == 1 {
                                pacenote_ambiguous_pick = None;
                            } else {
                                let prev_slug = ui
                                    .candidates
                                    .get(ui.index)
                                    .map(|c| c.slug.clone());
                                ui.candidates =
                                    hits_menu.iter().map(|(_, pick)| pick.clone()).collect();
                                ui.index = prev_slug
                                    .and_then(|slug| {
                                        ui.candidates.iter().position(|c| c.slug == slug)
                                    })
                                    .unwrap_or(0)
                                    .min(ui.candidates.len().saturating_sub(1));
                                match ui.keys.poll() {
                                    Some(PacenotePickerNav::Prev) => {
                                        let n = ui.candidates.len().max(1);
                                        ui.index = (ui.index + n - 1) % n;
                                    }
                                    Some(PacenotePickerNav::Next) => {
                                        let n = ui.candidates.len().max(1);
                                        ui.index = (ui.index + 1) % n;
                                    }
                                    Some(PacenotePickerNav::Confirm) => {
                                        let pick = ui.candidates[ui.index].clone();
                                        pacenote_ambiguous_pick = None;
                                        last_pacenote_ambiguous_where_voice_at = None;
                                        last_pacenote_ambiguous_help_log_at = None;
                                        if let Some(player) = pacenote_player.as_ref() {
                                            player.enqueue(
                                                vec![
                                                    acr_pacenote::pacenote_voice::PACENOTE_VOICE_FOUND_PACENOTES_TOKEN
                                                        .to_string(),
                                                ],
                                                0,
                                            );
                                        }
                                        if locked_track.is_none() {
                                            activate_standstill_track_lock(
                                                pick.reference_track.as_str(),
                                                &car_model_now,
                                                p,
                                                refs,
                                                &sector_sets,
                                                &timing_conn,
                                                &cfg.stage_timing,
                                                &mut timing_sector_cache,
                                                &mut active_timing_stage_slug,
                                                &mut locked_track,
                                                &mut locked_car_model,
                                                &mut active_track_name,
                                                &mut stable_selected,
                                                &mut timing_state,
                                                &mut sector_status_line,
                                                &mut detected_track_line,
                                                &mut last_sector_wait_log,
                                                &mut history,
                                                &mut last_pt,
                                                &mut total_drive_m,
                                                &format!(
                                                    "track locked from pacenote menu: {} ({})",
                                                    pick.reference_track, pick.slug
                                                ),
                                            );
                                        }
                                        apply_pacenote_first_anchor_resolution(
                                            &pick,
                                            &mut active_pacenote_stage_path,
                                            pc,
                                            locked_track.as_deref(),
                                            active_track_name.as_deref(),
                                            &mut pacenote_course,
                                            &mut pacenote_course_track,
                                            &mut triggered_pacenotes,
                                            &mut last_pacenote_gear_eval,
                                            &mut pacenote_gear_extra_lead_sec,
                                            &mut pacenote_manual_anchor_slug,
                                            &mut pacenote_loaded_src_mtime,
                                        );
                                        pacenote_manual_anchor_slug = Some(pick.slug.clone());
                                    }
                                    None => {}
                                }
                            }
                        }
                        let skip_ambiguous_reopen = pacenote_manual_anchor_slug.as_ref().is_some_and(|slug| {
                            hits_menu.iter().any(|(_, p)| p.slug == *slug)
                        });
                        if pacenote_ambiguous_pick.is_none() && !skip_ambiguous_reopen {
                            const PACENOTE_AMBIGUOUS_WHERE_COOLDOWN: Duration = Duration::from_secs(8);
                            let now_menu = Instant::now();
                            if explicit_multi && !hits_menu.is_empty() {
                                let n_hits = hits_menu.len();
                                if n_hits > 1 {
                                    let allow_where = last_pacenote_ambiguous_where_voice_at
                                        .map(|t| {
                                            now_menu.duration_since(t) >= PACENOTE_AMBIGUOUS_WHERE_COOLDOWN
                                        })
                                        .unwrap_or(true);
                                    if allow_where {
                                        if let Some(player) = pacenote_player.as_ref() {
                                            player.enqueue(
                                                vec![
                                                    acr_pacenote::pacenote_voice::PACENOTE_VOICE_WHERE_DO_WE_GO_TOKEN
                                                        .to_string(),
                                                ],
                                                0,
                                            );
                                        }
                                        last_pacenote_ambiguous_where_voice_at = Some(now_menu);
                                    }
                                    let allow_log = last_pacenote_ambiguous_help_log_at
                                        .map(|t| {
                                            now_menu.duration_since(t) >= PACENOTE_AMBIGUOUS_WHERE_COOLDOWN
                                        })
                                        .unwrap_or(true);
                                    if allow_log {
                                        eprintln!(
                                            "pacenote: {} configured stages within menu r={:.0} m (ref_geojson_candidates) — RTSS; Ctrl+arrows, Ctrl+Enter",
                                            n_hits, menu_r
                                        );
                                        last_pacenote_ambiguous_help_log_at = Some(now_menu);
                                    }
                                }
                                pacenote_ambiguous_pick = Some(AmbiguousPacenoteOverlayState {
                                    candidates: hits_menu
                                        .into_iter()
                                        .map(|(_, pick)| pick)
                                        .collect(),
                                    index: 0,
                                    keys: PacenotePickerKeyTracker::new(),
                                });
                            } else if hits_menu.len() > 1 {
                                let n_hits = hits_menu.len();
                                let allow_where = last_pacenote_ambiguous_where_voice_at
                                    .map(|t| {
                                        now_menu.duration_since(t) >= PACENOTE_AMBIGUOUS_WHERE_COOLDOWN
                                    })
                                    .unwrap_or(true);
                                if allow_where {
                                    if let Some(player) = pacenote_player.as_ref() {
                                        player.enqueue(
                                            vec![
                                                acr_pacenote::pacenote_voice::PACENOTE_VOICE_WHERE_DO_WE_GO_TOKEN
                                                    .to_string(),
                                            ],
                                            0,
                                        );
                                    }
                                    last_pacenote_ambiguous_where_voice_at = Some(now_menu);
                                }
                                let allow_log = last_pacenote_ambiguous_help_log_at
                                    .map(|t| {
                                        now_menu.duration_since(t) >= PACENOTE_AMBIGUOUS_WHERE_COOLDOWN
                                    })
                                    .unwrap_or(true);
                                if allow_log {
                                    eprintln!(
                                        "pacenote: {} first anchors within {:.0} m (menu r={:.0} m) — RTSS; Ctrl+arrows, Ctrl+Enter",
                                        n_hits,
                                        lock_r,
                                        menu_r
                                    );
                                    last_pacenote_ambiguous_help_log_at = Some(now_menu);
                                }
                                pacenote_ambiguous_pick = Some(AmbiguousPacenoteOverlayState {
                                    candidates: hits_menu.into_iter().map(|(_, pick)| pick).collect(),
                                    index: 0,
                                    keys: PacenotePickerKeyTracker::new(),
                                });
                            } else if hits_menu.len() == 1 {
                                let pick1 = &hits_menu[0].1;
                                if locked_track.is_none() {
                                    activate_standstill_track_lock(
                                        pick1.reference_track.as_str(),
                                        &car_model_now,
                                        p,
                                        refs,
                                        &sector_sets,
                                        &timing_conn,
                                        &cfg.stage_timing,
                                        &mut timing_sector_cache,
                                        &mut active_timing_stage_slug,
                                        &mut locked_track,
                                        &mut locked_car_model,
                                        &mut active_track_name,
                                        &mut stable_selected,
                                        &mut timing_state,
                                        &mut sector_status_line,
                                        &mut detected_track_line,
                                        &mut last_sector_wait_log,
                                        &mut history,
                                        &mut last_pt,
                                        &mut total_drive_m,
                                        &format!(
                                            "track locked from pacenote first-anchor (menu r={:.0} m): {}",
                                            menu_r, pick1.reference_track
                                        ),
                                    );
                                }
                                apply_pacenote_first_anchor_resolution(
                                    pick1,
                                    &mut active_pacenote_stage_path,
                                    pc,
                                    locked_track.as_deref(),
                                    active_track_name.as_deref(),
                                    &mut pacenote_course,
                                    &mut pacenote_course_track,
                                    &mut triggered_pacenotes,
                                    &mut last_pacenote_gear_eval,
                                    &mut pacenote_gear_extra_lead_sec,
                                    &mut pacenote_manual_anchor_slug,
                                    &mut pacenote_loaded_src_mtime,
                                );
                                pacenote_manual_anchor_slug = None;
                            }
                        }
                    }
                }
            }
            }
            if let Some(lp) = last_pt {
                total_drive_m += dist(lp, p);
            }
            if last_pt.map_or(true, |lp| dist(lp, p) > 0.05) {
                history.push_back(p);
                if history.len() > cfg.history_points {
                    history.pop_front();
                }
                if let Some(track_name) = &active_track_name {
                    if let Some(set) = sector_sets.get(track_name) {
                        if timing_state.is_none() {
                            timing_state = Some(LiveTimingState::new(set.ring_ids.clone()));
                            if let Some(state) = timing_state.as_mut() {
                                seed_sector_tracker_at_position(state, set, p);
                            }
                        }
                        if let Some(state) = timing_state.as_mut() {
                            if state.overall_markers.is_none() {
                                if let Some(path) = active_pacenote_stage_path.as_deref() {
                                    attach_stage_overall_markers(
                                        state,
                                        Some(path),
                                        &mut overall_marker_cache,
                                    );
                                    if let Some(m) = state.overall_markers.as_ref() {
                                        eprintln!(
                                            "overall markers: {} (route {:.0} m start→finish)",
                                            m.stage_slug, m.overall_route_lookup_m
                                        );
                                    }
                                }
                            }
                            ensure_stage_timing_sectors(
                                state,
                                track_name,
                                &cfg.stage_timing,
                                &mut timing_sector_cache,
                                &mut active_timing_stage_slug,
                                Some((p.x, p.z)),
                            );
                            let now_inst = Instant::now();
                            observe_active_leg_stats(state, &data.physics);
                            let rpm_now = data.physics.rpm as f64;
                            // Start staging: hold rpm > threshold while nearly stationary at same place.
                            if !state.start_armed {
                                if speed_kmh_now <= START_STAGE_SPEED_MAX && rpm_now >= START_STAGE_RPM_MIN {
                                    if let Some(sp) = state.start_stage_pos {
                                        if dist(sp, p) <= START_STAGE_RADIUS_M {
                                            if let Some(since) = state.start_stage_since {
                                                let held_sec = since.elapsed().as_secs_f64();
                                                let held_whole = held_sec.floor() as i32;
                                                if held_whole != state.start_stage_last_report_sec {
                                                    state.start_stage_last_report_sec = held_whole;
                                                    let remaining = (START_STAGE_HOLD_SEC - held_sec).max(0.0);
                                                    let line = format!(
                                                        "start staging... rpm>{:.0}, v<{:.1} ({:.1}s left)",
                                                        START_STAGE_RPM_MIN,
                                                        START_STAGE_SPEED_MAX,
                                                        remaining
                                                    );
                                                    sector_status_line = Some((line, Instant::now()));
                                                }
                                            }
                                            if state
                                                .start_stage_since
                                                .map(|t| t.elapsed().as_secs_f64() >= START_STAGE_HOLD_SEC)
                                                .unwrap_or(false)
                                            {
                                                state.start_armed = true;
                                                state.leg_stats.reset();
                                                set_leg_entry_speed(state, data.physics.speed_kmh);
                                                state.start_anchor_t_sec = Some(data.graphics.clock as f64);
                                                state.start_anchor_instant = Some(now_inst);
                                                state.start_anchor_drive_m = Some(total_drive_m);
                                                let car_model = if car_model_now.is_empty() {
                                                    "unknown_car"
                                                } else {
                                                    car_model_now.as_str()
                                                };
                                                if let Err(e) = append_start_point_geojson(
                                                    &cfg.start_points_geojson,
                                                    track_name,
                                                    car_model,
                                                    p,
                                                ) {
                                                    eprintln!("start geojson append failed: {}", e);
                                                }
                                                let line = "sector [Start]...".to_string();
                                                eprintln!("start armed: {}", line);
                                                sector_status_line = Some((line, Instant::now()));
                                            }
                                        } else {
                                            state.start_stage_pos = Some(p);
                                            state.start_stage_since = Some(now_inst);
                                            state.start_stage_last_report_sec = -1;
                                            let line = format!(
                                                "start candidate: hold rpm>{:.0}, v<{:.1}",
                                                START_STAGE_RPM_MIN, START_STAGE_SPEED_MAX
                                            );
                                            sector_status_line = Some((line, Instant::now()));
                                        }
                                    } else {
                                        state.start_stage_pos = Some(p);
                                        state.start_stage_since = Some(now_inst);
                                        state.start_stage_last_report_sec = -1;
                                        let line = format!(
                                            "start candidate: hold rpm>{:.0}, v<{:.1}",
                                            START_STAGE_RPM_MIN, START_STAGE_SPEED_MAX
                                        );
                                        sector_status_line = Some((line, Instant::now()));
                                    }
                                } else {
                                    state.start_stage_pos = None;
                                    state.start_stage_since = None;
                                    state.start_stage_last_report_sec = -1;
                                }
                            } else if speed_kmh_now >= START_TRIGGER_SPEED_KMH {
                                // Keep start anchor armed until first real sector crossing consumes it.
                                if state.start_anchor_instant.is_none() {
                                    state.start_anchor_t_sec = Some(data.graphics.clock as f64);
                                    state.start_anchor_instant = Some(now_inst);
                                    state.start_anchor_drive_m = Some(total_drive_m);
                                    set_leg_entry_speed(state, data.physics.speed_kmh);
                                }
                            }
                            if !state.overall_finish_recorded {
                                if let Some(markers) = state.overall_markers.as_ref() {
                                    if let Some((_start, finish)) =
                                        acr_timing::stage_overall_markers::start_finish(markers)
                                    {
                                        let finish_p = Point2 {
                                            x: finish.x,
                                            z: finish.z,
                                        };
                                        if dist(p, finish_p) <= OVERALL_MARKER_RADIUS_M {
                                            if let (Some(st), Some(si), Some(sm)) = (
                                                state.start_anchor_t_sec,
                                                state.start_anchor_instant,
                                                state.start_anchor_drive_m,
                                            ) {
                                                let mut dt = data.graphics.clock as f64 - st;
                                                if dt < 0.0 {
                                                    dt += 24.0 * 3600.0;
                                                }
                                                let dt = si.elapsed().as_secs_f64().max(dt);
                                                if dt > 0.05 {
                                                    let car_model = data.statics.car_model.trim();
                                                    let car_model = if car_model.is_empty() {
                                                        "unknown_car"
                                                    } else {
                                                        car_model
                                                    };
                                                    let exit_speed = data.physics.speed_kmh;
                                                    let leg_stats = take_leg_stats(state, exit_speed);
                                                    let direction_s = state
                                                        .tracker
                                                        .locked_direction()
                                                        .map(|d| match d {
                                                            SectorTravelDirection::Increasing => {
                                                                "inc"
                                                            }
                                                            SectorTravelDirection::Decreasing => {
                                                                "dec"
                                                            }
                                                        })
                                                        .unwrap_or("inc");
                                                    let split =
                                                        acr_timing::timing_db::SplitRecord {
                                                            track_name,
                                                            car_model,
                                                            direction: direction_s,
                                                            from_sector: START_SECTOR_ID,
                                                            to_sector: FINISH_SECTOR_ID,
                                                            duration_sec: dt,
                                                            distance_m: (total_drive_m - sm)
                                                                .max(0.0),
                                                            stats: leg_stats,
                                                        };
                                                    let (line, delta) =
                                                        if let Some(locked) =
                                                            locked_track.as_deref()
                                                        {
                                                            if locked == track_name {
                                                                persist_split_and_line(
                                                                    &timing_conn,
                                                                    &split,
                                                                )
                                                            } else {
                                                                let _ = acr_timing::timing_db::insert_pending_split(
                                                                    &timing_conn,
                                                                    &split,
                                                                );
                                                                (
                                                                    format!(
                                                                        "overall [Start]-[Finish]: {:.3}s (pending)",
                                                                        dt
                                                                    ),
                                                                    0.0,
                                                                )
                                                            }
                                                        } else {
                                                            let _ = acr_timing::timing_db::insert_pending_split(
                                                                &timing_conn,
                                                                &split,
                                                            );
                                                            (
                                                                format!(
                                                                    "overall [Start]-[Finish]: {:.3}s (pending)",
                                                                    dt
                                                                ),
                                                                0.0,
                                                            )
                                                        };
                                                    eprintln!("{line}");
                                                    latest_timing_line =
                                                        Some((line, Instant::now()));
                                                    if cfg.beep_on_split {
                                                        acr_timing::split_beep::play_split_feedback(
                                                            delta,
                                                            &cfg.split_beep,
                                                        );
                                                    }
                                                    state.overall_finish_recorded = true;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some(lp) = last_pt.as_ref().copied() {
                                let step_m = dist(lp, p);
                                if step_m <= MAX_SECTOR_CROSS_SEGMENT_M {
                                    if let Some(session) = state.stage_sector_session.as_mut() {
                                        let now_inst = Instant::now();
                                        let clock_sec = data.graphics.clock as f64;
                                        let car_model = data.statics.car_model.trim();
                                        let car_model = if car_model.is_empty() {
                                            "unknown_car"
                                        } else {
                                            car_model
                                        };
                                        let stage_radius =
                                            cfg.stage_timing.stage_sector_radius_m();
                                        if let Some(outcome) =
                                            acr_timing::stage_sector_timing::observe_stage_crossing(
                                                session,
                                                (lp.x, lp.z),
                                                (p.x, p.z),
                                                stage_radius,
                                                clock_sec,
                                                now_inst,
                                            )
                                        {
                                            if let Some(crossed) = session
                                                .markers
                                                .markers
                                                .iter()
                                                .find(|m| m.order == outcome.to_order)
                                            {
                                                let via = outcome
                                                    .pass_method
                                                    .map(|m| match m {
                                                        acr_timing::timing_sectors::GatePassMethod::GateLine => "gate_line",
                                                        acr_timing::timing_sectors::GatePassMethod::RadiusDisc => "radius_disc",
                                                    })
                                                    .unwrap_or("?");
                                                acr_timing::physics_wheel::log_sector_crossing(
                                                    &crossed.label,
                                                    crossed.role.as_str(),
                                                    via,
                                                    &data.physics,
                                                    p.x,
                                                    p.z,
                                                    speed_kmh_now,
                                                    outcome.leg_duration_sec,
                                                    data.graphics.distance_traveled,
                                                    crossed.x,
                                                    crossed.z,
                                                );
                                            }
                                            if let Some(dt) = outcome.leg_duration_sec {
                                                match acr_timing::stage_sector_timing::persist_stage_leg(
                                                    &timing_conn,
                                                    &session.markers.stage_slug,
                                                    car_model,
                                                    outcome.from_order,
                                                    outcome.to_order,
                                                    dt,
                                                ) {
                                                    Ok(delta) => {
                                                        if let Some(detail) =
                                                            outcome.overlay_detail.clone()
                                                        {
                                                            eprintln!("{detail}");
                                                            latest_timing_line =
                                                                Some((detail.clone(), now_inst));
                                                            last_overlay_push = Instant::now()
                                                                - Duration::from_secs(5);
                                                        }
                                                        if cfg.beep_on_split {
                                                            acr_timing::split_beep::play_split_feedback(
                                                                delta, &cfg.split_beep,
                                                            );
                                                        }
                                                    }
                                                    Err(e) => {
                                                        eprintln!("stage sector DB: {}", e)
                                                    }
                                                }
                                            } else if let Some(detail) =
                                                outcome.overlay_detail.clone()
                                            {
                                                eprintln!("{detail}");
                                                sector_status_line =
                                                    Some((detail, now_inst));
                                                last_overlay_push = Instant::now()
                                                    - Duration::from_secs(5);
                                            }
                                            if outcome.run_completed {
                                                flush_stage_sector_run(
                                                    state,
                                                    &cfg,
                                                    car_model,
                                                    &mut stage_sector_html_run_counter,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            seed_sector_tracker_at_position(state, set, p);
                            if let Some(lp) = last_pt.as_ref().copied() {
                                let step_m = dist(lp, p);
                                if step_m > MAX_SECTOR_CROSS_SEGMENT_M {
                                    eprintln!(
                                        "sector cross skipped: step {:.0} m (> {:.0} m), reset last position",
                                        step_m, MAX_SECTOR_CROSS_SEGMENT_M
                                    );
                                    last_pt = Some(p);
                                } else if state.tracker.current_sector().is_some()
                                    && let Some((cross_idx, _t)) = first_crossed_sector(
                                        lp,
                                        p,
                                        &set.boundaries,
                                        cfg.sector_search_radius_m,
                                    )
                                {
                                    let now = Instant::now();
                                    if state
                                        .cooldown_until
                                        .get(&cross_idx)
                                        .map_or(false, |until| now < *until)
                                    {
                                        // still cooling down for this sector, ignore
                                    } else {
                                        state.cooldown_until.insert(
                                            cross_idx,
                                            now + Duration::from_millis(cfg.sector_cross_cooldown_ms),
                                        );
                                        match state.tracker.observe(cross_idx) {
                                            SectorPassEvent::Anchored { sector } => {
                                                // If a staged start exists, emit Start->first-sector split now.
                                                if state.start_armed {
                                                    if let (Some(st), Some(si), Some(sm)) = (
                                                        state.start_anchor_t_sec,
                                                        state.start_anchor_instant,
                                                        state.start_anchor_drive_m,
                                                    ) {
                                                        let mut dt = data.graphics.clock as f64 - st;
                                                        if dt < 0.0 {
                                                            dt += 24.0 * 3600.0;
                                                        }
                                                        let dt = si.elapsed().as_secs_f64().max(dt);
                                                        if dt > 0.05 {
                                                            let to_sector_id = state.ring_ids[sector];
                                                            let direction_s = state
                                                                .tracker
                                                                .locked_direction()
                                                                .map(|d| match d {
                                                                    SectorTravelDirection::Increasing => "inc",
                                                                    SectorTravelDirection::Decreasing => "dec",
                                                                })
                                                                .unwrap_or("inc");
                                                            let car_model =
                                                                data.statics.car_model.trim();
                                                            let car_model = if car_model.is_empty() {
                                                                "unknown_car"
                                                            } else {
                                                                car_model
                                                            };
                                                            let exit_speed = data.physics.speed_kmh;
                                                            let leg_stats = take_leg_stats(state, exit_speed);
                                                            let split = acr_timing::timing_db::SplitRecord {
                                                                track_name,
                                                                car_model,
                                                                direction: direction_s,
                                                                from_sector: START_SECTOR_ID,
                                                                to_sector: to_sector_id,
                                                                duration_sec: dt,
                                                                distance_m: (total_drive_m - sm).max(0.0),
                                                                stats: leg_stats,
                                                            };
                                                            let (line, delta) = if let Some(locked) =
                                                                locked_track.as_deref()
                                                            {
                                                                if locked == track_name {
                                                                    persist_split_and_line(
                                                                        &timing_conn,
                                                                        &split,
                                                                    )
                                                                } else {
                                                                    let _ =
                                                                        acr_timing::timing_db::insert_pending_split(
                                                                            &timing_conn,
                                                                            &split,
                                                                        );
                                                                    (
                                                                        format!(
                                                                            "sector [Start]-[{}]: {:.3}s (pending)",
                                                                            to_sector_id, dt
                                                                        ),
                                                                        0.0,
                                                                    )
                                                                }
                                                            } else {
                                                                let _ = acr_timing::timing_db::insert_pending_split(
                                                                    &timing_conn,
                                                                    &split,
                                                                );
                                                                (
                                                                    format!(
                                                                        "sector [Start]-[{}]: {:.3}s (pending)",
                                                                        to_sector_id, dt
                                                                    ),
                                                                    0.0,
                                                                )
                                                            };
                                                            eprintln!("{line}");
                                                            latest_timing_line = Some((line, Instant::now()));
                                                            if cfg.beep_on_split {
                                                                acr_timing::split_beep::play_split_feedback(
                                                                    delta,
                                                                    &cfg.split_beep,
                                                                );
                                                            }
                                                        }
                                                    }
                                                    state.start_armed = false;
                                                    state.start_anchor_t_sec = None;
                                                    state.start_anchor_instant = None;
                                                    state.start_anchor_drive_m = None;
                                                    state.start_stage_pos = None;
                                                    state.start_stage_since = None;
                                                    state.start_stage_last_report_sec = -1;
                                                }
                                                state.last_anchor_t_sec = Some(data.graphics.clock as f64);
                                                state.last_anchor_instant = Some(Instant::now());
                                                state.last_anchor_drive_m = Some(total_drive_m);
                                                state.last_sector_idx = Some(sector);
                                                state.leg_stats.reset();
                                                set_leg_entry_speed(state, data.physics.speed_kmh);
                                                let anchor_line =
                                                    format!("sector [{}]...", state.ring_ids[sector]);
                                                eprintln!("{}", anchor_line);
                                                sector_status_line = Some((anchor_line, Instant::now()));
                                            }
                                            SectorPassEvent::Step { from, to, direction } => {
                                                let now_t = data.graphics.clock as f64;
                                                let now_inst = Instant::now();
                                                if let (Some(prev_t), Some(prev_m)) =
                                                    (state.last_anchor_t_sec, state.last_anchor_drive_m)
                                                {
                                                    let dt = state
                                                        .last_anchor_instant
                                                        .map(|t| now_inst.duration_since(t).as_secs_f64())
                                                        .unwrap_or_else(|| {
                                                            let mut x = now_t - prev_t;
                                                            if x < 0.0 {
                                                                x += 24.0 * 3600.0;
                                                            }
                                                            x
                                                        });
                                                    let dist_m = (total_drive_m - prev_m).max(0.0);
                                                    if dt > 0.05 {
                                                        let from_sector_id = state.ring_ids[from];
                                                        let to_sector_id = state.ring_ids[to];
                                                        let direction_s = match direction {
                                                            SectorTravelDirection::Increasing => "inc",
                                                            SectorTravelDirection::Decreasing => "dec",
                                                        };
                                                        let car_model =
                                                            data.statics.car_model.trim();
                                                        let car_model = if car_model.is_empty() {
                                                            "unknown_car"
                                                        } else {
                                                            car_model
                                                        };

                                                        let exit_speed = data.physics.speed_kmh;
                                                        let leg_stats = take_leg_stats(state, exit_speed);
                                                        let split = acr_timing::timing_db::SplitRecord {
                                                            track_name,
                                                            car_model,
                                                            direction: direction_s,
                                                            from_sector: from_sector_id,
                                                            to_sector: to_sector_id,
                                                            duration_sec: dt,
                                                            distance_m: dist_m,
                                                            stats: leg_stats,
                                                        };
                                                        let (line, delta) = if let Some(locked) =
                                                            locked_track.as_deref()
                                                        {
                                                            if locked == track_name {
                                                                persist_split_and_line(
                                                                    &timing_conn,
                                                                    &split,
                                                                )
                                                            } else {
                                                                let _ = acr_timing::timing_db::insert_pending_split(
                                                                    &timing_conn,
                                                                    &split,
                                                                );
                                                                (
                                                                    format!(
                                                                        "sector [{}]-[{}]: {:.3}s (pending)",
                                                                        from_sector_id, to_sector_id, dt
                                                                    ),
                                                                    0.0,
                                                                )
                                                            }
                                                        } else {
                                                            let _ = acr_timing::timing_db::insert_pending_split(
                                                                &timing_conn,
                                                                &split,
                                                            );
                                                            (
                                                                format!(
                                                                    "sector [{}]-[{}]: {:.3}s (pending)",
                                                                    from_sector_id, to_sector_id, dt
                                                                ),
                                                                0.0,
                                                            )
                                                        };
                                                        eprintln!("{line}");
                                                        latest_timing_line = Some((line.clone(), Instant::now()));
                                                        if cfg.beep_on_split {
                                                            acr_timing::split_beep::play_split_feedback(
                                                                delta,
                                                                &cfg.split_beep,
                                                            );
                                                        }
                                                    }
                                                }
                                                eprintln!("sector passed [{}]", state.ring_ids[to]);
                                                if active_track_name.is_some() {
                                                    let passed_line = format!("sector passed [{}]", state.ring_ids[to]);
                                                    sector_status_line = Some((passed_line.clone(), Instant::now()));
                                                }
                                                state.last_anchor_t_sec = Some(now_t);
                                                state.last_anchor_instant = Some(now_inst);
                                                state.last_anchor_drive_m = Some(total_drive_m);
                                                state.last_sector_idx = Some(to);
                                                set_leg_entry_speed(state, data.physics.speed_kmh);
                                            }
                                            SectorPassEvent::NoStep { .. }
                                            => {
                                                // Typical restart case: same sector crossed again after a pause.
                                                // Re-anchor timing to avoid carrying over a stale start timestamp.
                                                let now_inst2 = Instant::now();
                                                let now_t2 = data.graphics.clock as f64;
                                                let should_reanchor = state
                                                    .last_anchor_instant
                                                    .map(|t| now_inst2.duration_since(t).as_secs_f64() >= SAME_SECTOR_REANCHOR_SEC)
                                                    .unwrap_or(true);
                                                if should_reanchor {
                                                    state.last_anchor_t_sec = Some(now_t2);
                                                    state.last_anchor_instant = Some(now_inst2);
                                                    state.last_anchor_drive_m = Some(total_drive_m);
                                                    state.leg_stats.reset();
                                                    set_leg_entry_speed(state, data.physics.speed_kmh);
                                                    if let Some(si) = state.last_sector_idx {
                                                        let line = format!("sector [{}]...", state.ring_ids[si]);
                                                        eprintln!("re-anchored at same sector: {}", line);
                                                        sector_status_line = Some((line, Instant::now()));
                                                    }
                                                }
                                            }
                                            SectorPassEvent::Unexpected { .. }
                                            | SectorPassEvent::DirectionConflict { .. } => {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(pacenote_cfg) = pacenote_cfg.as_ref() {
                    if speed_kmh_now >= pacenote_cfg.min_speed_kmh {
                        if let Some(track_name) = locked_track.as_deref() {
                            let pacenote_sel = active_pacenote_stage_path.clone();
                            attach_pacenote_course_for_track(
                                pacenote_cfg,
                                track_name,
                                pacenote_sel.as_deref(),
                                &mut active_pacenote_stage_path,
                                Some((p.x, p.z)),
                                pacenote_stage_catalog.as_ref(),
                                &mut pacenote_course,
                                &mut pacenote_course_track,
                                &mut triggered_pacenotes,
                                &mut pacenote_loaded_src_mtime,
                            );
                            if let Some(course) = pacenote_course.as_ref() {
                                let gear_eval_interval = Duration::from_millis(
                                    (1000 / pacenote_cfg.gear_advance_hz.max(1)) as u64,
                                );
                                if last_pacenote_gear_eval.elapsed() >= gear_eval_interval {
                                    last_pacenote_gear_eval = Instant::now();
                                    pacenote_gear_extra_lead_sec = 0.0;
                                    if data.physics.gear >= pacenote_cfg.gear_advance_gear {
                                        if let Some(pos) =
                                            course.next_callout_pos(&triggered_pacenotes)
                                        {
                                            if let (Some(driving), Some(corner)) = (
                                                pacenote_course::driving_gear(
                                                    data.physics.gear,
                                                ),
                                                course.callouts[pos].max_turn_severity,
                                            ) {
                                                pacenote_gear_extra_lead_sec =
                                                    pacenote_course::gear_extra_lead_sec(
                                                        driving,
                                                        corner,
                                                        pacenote_cfg.gear_reference_severity,
                                                        pacenote_cfg.gear_step_ms,
                                                    );
                                            }
                                        }
                                    }
                                }
                            }
                            if let (Some(course), Some(player)) =
                                (pacenote_course.as_ref(), pacenote_player.as_ref())
                            {
                                if let Some(pos) =
                                    course.next_callout_pos(&triggered_pacenotes)
                                {
                                    let callout = &course.callouts[pos];
                                    let chain_end = course.callout_chain_end_pos(pos);
                                    let (tokens, indices) =
                                        collect_callout_tokens(course, pos);
                                    let callout_urgency =
                                        course.callout_chain_urgency(pos, chain_end);
                                    let next_urgency = course
                                        .callouts
                                        .get(chain_end + 1)
                                        .and_then(|next| next.max_turn_severity);
                                    let time_to_next_callout_sec = if speed_kmh_now > 0.0 {
                                        course.leg_distance_to_next(chain_end)
                                            / (speed_kmh_now / 3.6)
                                    } else {
                                        f64::INFINITY
                                    };
                                    let voice_dir = pacenote_cfg
                                        .voice_dir
                                        .as_deref()
                                        .unwrap_or(Path::new("."));
                                    let conflict_advance_sec =
                                        acr_pacenote::pacenote_voice::conflict_lead_advance_sec(
                                            voice_dir,
                                            &tokens,
                                            callout_urgency,
                                            next_urgency,
                                            time_to_next_callout_sec,
                                            pacenote_cfg,
                                        );
                                    let slow_corner_extra = callout
                                        .max_turn_severity
                                        .filter(|severity| {
                                            *severity <= pacenote_cfg.protected_corner_gear
                                        })
                                        .map(|_| pacenote_cfg.slow_corner_extra_lead_sec)
                                        .unwrap_or(0.0);
                                    let lead_sec_base = pacenote_cfg.lead_sec;
                                    let lookahead_uncapped_m = pacenote_course::lead_distance_m(
                                        speed_kmh_now,
                                        lead_sec_base + slow_corner_extra + conflict_advance_sec,
                                        pacenote_gear_extra_lead_sec,
                                    );
                                    let leg_to_next_m =
                                        course.leg_distance_for_lookahead_cap(pos, chain_end);
                                    let lookahead_m = pacenote_course::capped_lookahead_m(
                                        lookahead_uncapped_m,
                                        leg_to_next_m,
                                        pacenote_cfg.skip_buffer_m,
                                    );
                                    let distance_m =
                                        course.route_distance_to_callout((p.x, p.z), pos);
                                    let ahead = pacenote_course::should_trigger_ahead(
                                        distance_m,
                                        lookahead_m,
                                    );
                                    let passed = last_pt
                                        .map(|lp| {
                                            pacenote_course::crossed_callout(
                                                (lp.x, lp.z),
                                                (p.x, p.z),
                                                callout,
                                                pacenote_cfg.trigger_radius_m,
                                            )
                                        })
                                        .unwrap_or(false);
                                    if ahead || passed {
                                        log_pacenote_enqueue_lead(
                                            callout,
                                            speed_kmh_now,
                                            data.physics.gear,
                                            pacenote_cfg,
                                            lead_sec_base,
                                            slow_corner_extra,
                                            conflict_advance_sec,
                                            pacenote_gear_extra_lead_sec,
                                            lookahead_m,
                                            lookahead_uncapped_m,
                                            leg_to_next_m,
                                            distance_m,
                                            ahead,
                                            passed,
                                            callout_urgency,
                                            next_urgency,
                                            time_to_next_callout_sec,
                                        );
                                        player.enqueue(tokens, callout_urgency);
                                        eprintln!(
                                            "pacenote [{}]: {}",
                                            callout.index, callout.notes_text
                                        );
                                        for idx in indices {
                                            triggered_pacenotes.insert(idx);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                last_pt = Some(p);
            }
            if last_eval.elapsed() >= eval_interval {
                if locked_track.is_none() && !start_points_mode {
                    if let Some(st) = select_track_from_starts(
                        &start_index,
                        p,
                        cfg.start_prefilter_radius_m,
                    )
                    .and_then(|st| {
                        filter_tracks_by_spline_length(
                            observed_spline_m,
                            vec![st],
                            &spline_catalog,
                        )
                        .into_iter()
                        .next()
                    }) {
                        if refs.iter().any(|r| r.name == st) {
                            activate_standstill_track_lock(
                                &st,
                                &car_model_now,
                                p,
                                refs,
                                &sector_sets,
                                &timing_conn,
                                &cfg.stage_timing,
                                &mut timing_sector_cache,
                                &mut active_timing_stage_slug,
                                &mut locked_track,
                                &mut locked_car_model,
                                &mut active_track_name,
                                &mut stable_selected,
                                &mut timing_state,
                                &mut sector_status_line,
                                &mut detected_track_line,
                                &mut last_sector_wait_log,
                                &mut history,
                                &mut last_pt,
                                &mut total_drive_m,
                                &format!(
                                    "track locked from start_points.geojson (unique within {:.0} m, spline {:.1} m): {}",
                                    cfg.start_prefilter_radius_m, observed_spline_m, st
                                ),
                            );
                            if let Some(catalog) = pacenote_stage_catalog.as_ref() {
                                if let Some(pick) = catalog.select_from_position(
                                    p.x,
                                    p.z,
                                    cfg.start_prefilter_radius_m,
                                ) {
                                    if pick.reference_track == st {
                                        active_pacenote_stage_path = Some(pick.path.clone());
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(locked_name) = locked_track.as_deref() {
                    if active_track_name.as_deref() != Some(locked_name) {
                        active_track_name = Some(locked_name.to_string());
                    }
                    if timing_state.is_none() {
                        if let Some(s) = sector_sets.get(locked_name) {
                            timing_state = Some(LiveTimingState::new(s.ring_ids.clone()));
                            if let Some(state) = timing_state.as_mut() {
                                seed_sector_tracker_at_position(state, s, p);
                                ensure_stage_timing_sectors(
                                    state,
                                    locked_name,
                                    &cfg.stage_timing,
                                    &mut timing_sector_cache,
                                    &mut active_timing_stage_slug,
                                    Some((p.x, p.z)),
                                );
                            }
                        }
                    } else if let Some(state) = timing_state.as_mut() {
                        ensure_stage_timing_sectors(
                            state,
                            locked_name,
                            &cfg.stage_timing,
                            &mut timing_sector_cache,
                            &mut active_timing_stage_slug,
                            Some((p.x, p.z)),
                        );
                    }
                    if last_sector_wait_log.elapsed() >= Duration::from_secs(5) {
                        eprintln!("track locked: {}", locked_name);
                        last_sector_wait_log = Instant::now();
                    }
                    let now_osd = Instant::now();
                    let (msg, osd_lines) =
                        if let Some(sess) = timing_state
                            .as_ref()
                            .and_then(|s| s.stage_sector_session.as_ref())
                        {
                            let mut detail = acr_timing::stage_sector_timing::stage_timing_osd_detail(
                                sess, p.x, p.z,
                            );
                            if let Some((line, ts)) = &latest_timing_line {
                                if ts.elapsed() <= Duration::from_secs(6) {
                                    detail = format!("{detail} | {line}");
                                }
                            } else if let Some((sline, sts)) = &sector_status_line {
                                if sts.elapsed() <= Duration::from_secs(6) {
                                    detail = format!("{detail} | {sline}");
                                }
                            }
                            let strip = acr_timing::stage_sector_timing::format_sector_strip(
                                &sess.run.sector_secs,
                                cfg.rtss,
                                sess.run.highlight_leg_index(),
                                sess.run.live_leg_elapsed_sec(now_osd),
                            );
                            (
                                acr_timing::stage_sector_timing::compose_stage_timing_osd(
                                    &format!("track locked {}", locked_name),
                                    &strip,
                                    &detail,
                                ),
                                3,
                            )
                        } else {
                            let detail = if let Some((line, _ts)) = &latest_timing_line {
                                line.to_string()
                            } else if let Some((sline, sts)) = &sector_status_line {
                                if sts.elapsed() <= Duration::from_secs(8) {
                                    sline.to_string()
                                } else {
                                    String::new()
                                }
                            } else {
                                String::new()
                            };
                            (
                                compose_two_line_osd(
                                    &format!("track locked {}", locked_name),
                                    &detail,
                                ),
                                2,
                            )
                        };
                    let osd_push_interval = if timing_state
                        .as_ref()
                        .and_then(|s| s.stage_sector_session.as_ref())
                        .is_some()
                    {
                        Duration::from_millis(400)
                    } else {
                        Duration::from_secs(2)
                    };
                    if msg != last_overlay_msg || last_overlay_push.elapsed() >= osd_push_interval {
                        push_live_overlay(cfg, &msg, osd_lines)?;
                        last_overlay_msg = msg;
                        last_overlay_push = Instant::now();
                    }
                    last_eval = Instant::now();
                    continue;
                }

                let query = live_match_query(&history, p, 21);
                let scores = match_tracks(&query, refs, cfg);
                if let Some(best) = scores.first() {
                    let start_pref = if start_points_mode {
                        None
                    } else {
                        select_track_from_starts(
                            &start_index,
                            p,
                            cfg.start_prefilter_radius_m,
                        )
                        .and_then(|st| {
                            filter_tracks_by_spline_length(
                                observed_spline_m,
                                vec![st],
                                &spline_catalog,
                            )
                            .into_iter()
                            .next()
                        })
                    };
                    if let Some(pref) = &start_pref {
                        eprintln!("start prefilter candidate: {}", pref);
                    }
                    let pacenote_start = pacenote_stage_catalog.as_ref().and_then(|catalog| {
                        catalog.select_from_position(p.x, p.z, cfg.start_prefilter_radius_m)
                    });
                    if let Some(pick) = &pacenote_start {
                        eprintln!(
                            "pacenote start candidate: {} ({})",
                            pick.reference_track, pick.slug
                        );
                        if locked_track.is_none()
                            && pacenote_ambiguous_pick.is_none()
                            && start_track_ambiguous_pick.is_none()
                            && !start_points_mode
                        {
                            active_pacenote_stage_path = Some(pick.path.clone());
                        }
                    }
                    let track_pref = pacenote_start
                        .as_ref()
                        .map(|pick| pick.reference_track.as_str())
                        .or(start_pref.as_deref());
                    // Track hysteresis: keep current track while still plausible and
                    // only switch when candidate is clearly better.
                    let selected = if let Some(pref_name) = track_pref {
                        if let Some(pref_score) = scores.iter().find(|s| s.name == pref_name) {
                            pref_score
                        } else {
                            best
                        }
                    } else if best.coarse_pass {
                        if let Some(active_name) = active_track_name.as_deref() {
                            if let Some(active_score) = scores.iter().find(|s| s.name == active_name) {
                                if active_score.coarse_pass
                                    && active_score.mean_dist_m <= cfg.track_keep_max_dist_m
                                    && best.name != active_name
                                {
                                    let gain = active_score.final_score - best.final_score;
                                    if gain < cfg.track_switch_min_gain {
                                        active_score
                                    } else {
                                        best
                                    }
                                } else {
                                    best
                                }
                            } else {
                                best
                            }
                        } else {
                            best
                        }
                    } else {
                        best
                    };

                    if selected.coarse_pass {
                        if !start_points_mode && locked_track.is_none() {
                            if let Some(pick) = &pacenote_start {
                                if pick.reference_track == selected.name {
                                    active_pacenote_stage_path = Some(pick.path.clone());
                                    activate_standstill_track_lock(
                                        selected.name.as_str(),
                                        &car_model_now,
                                        p,
                                        refs,
                                        &sector_sets,
                                        &timing_conn,
                                        &cfg.stage_timing,
                                        &mut timing_sector_cache,
                                        &mut active_timing_stage_slug,
                                        &mut locked_track,
                                        &mut locked_car_model,
                                        &mut active_track_name,
                                        &mut stable_selected,
                                        &mut timing_state,
                                        &mut sector_status_line,
                                        &mut detected_track_line,
                                        &mut last_sector_wait_log,
                                        &mut history,
                                        &mut last_pt,
                                        &mut total_drive_m,
                                        &format!(
                                            "track locked from pacenote start: {} ({})",
                                            selected.name, pick.slug
                                        ),
                                    );
                                }
                            }
                        }
                        if !start_points_mode && locked_track.is_none() {
                            if let Some((name, since)) = &stable_selected {
                                if name == &selected.name {
                                    if since.elapsed().as_secs_f64() >= cfg.track_lock_after_sec {
                                        activate_standstill_track_lock(
                                            selected.name.as_str(),
                                            &car_model_now,
                                            p,
                                            refs,
                                            &sector_sets,
                                            &timing_conn,
                                            &cfg.stage_timing,
                                            &mut timing_sector_cache,
                                            &mut active_timing_stage_slug,
                                            &mut locked_track,
                                            &mut locked_car_model,
                                            &mut active_track_name,
                                            &mut stable_selected,
                                            &mut timing_state,
                                            &mut sector_status_line,
                                            &mut detected_track_line,
                                            &mut last_sector_wait_log,
                                            &mut history,
                                            &mut last_pt,
                                            &mut total_drive_m,
                                            &format!(
                                                "track locked after {:.1}s stable geometry: {} (car={})",
                                                cfg.track_lock_after_sec,
                                                selected.name,
                                                if car_model_now.is_empty() {
                                                    "unknown"
                                                } else {
                                                    car_model_now.as_str()
                                                }
                                            ),
                                        );
                                    }
                                } else {
                                    stable_selected = Some((selected.name.clone(), Instant::now()));
                                }
                            } else {
                                stable_selected = Some((selected.name.clone(), Instant::now()));
                            }
                        }
                        if active_track_name.as_deref() != Some(selected.name.as_str()) {
                            active_track_name = Some(selected.name.clone());
                            timing_state = if let Some(s) = sector_sets.get(&selected.name) {
                                detected_track_line =
                                    Some((format!("detected track {}", selected.name), Instant::now()));
                                let mut ts = LiveTimingState::new(s.ring_ids.clone());
                                seed_sector_tracker_at_position(&mut ts, s, p);
                                let seg = ts
                                    .last_sector_idx
                                    .and_then(|i| ts.ring_ids.get(i).copied())
                                    .unwrap_or(-1);
                                let line = format!("sector at position [{}]", seg);
                                eprintln!("{} ({})", line, selected.name);
                                sector_status_line = Some((line, Instant::now()));
                                Some(ts)
                            } else {
                                let line = "no sector set for detected track".to_string();
                                eprintln!("{} ({})", line, selected.name);
                                sector_status_line = Some((line, Instant::now()));
                                detected_track_line =
                                    Some((format!("detected track {}", selected.name), Instant::now()));
                                None
                            };
                            last_sector_wait_log = Instant::now();
                        }
                    } else {
                        stable_selected = None;
                        active_track_name = None;
                        timing_state = None;
                        sector_status_line = None;
                        detected_track_line = None;
                    }
                    let status_line = if best.coarse_pass {
                        if let Some(active) = active_track_name.as_deref() {
                            if locked_track.as_deref() == Some(active) {
                                format!("track locked {}", active)
                            } else {
                                format!("track found {} (unlocked)", active)
                            }
                        } else {
                            "track found".to_string()
                        }
                    } else {
                        "detecting track...".to_string()
                    };
                    if best.coarse_pass && timing_state.is_some() && latest_timing_line.is_none() {
                        if last_sector_wait_log.elapsed() >= Duration::from_secs(3) {
                            eprintln!("waiting for sector passing...");
                            last_sector_wait_log = Instant::now();
                        }
                    }
                    let detail = if let Some((line, _ts)) = &latest_timing_line {
                        // Keep latest split sticky until replaced by newer split.
                        line.to_string()
                    } else if let Some((sline, sts)) = &sector_status_line {
                        if sts.elapsed() <= Duration::from_secs(8) {
                            sline.to_string()
                        } else if let Some((dline, dts)) = &detected_track_line {
                            if dts.elapsed() <= Duration::from_secs(5) {
                                format!("status: {}", dline)
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        }
                    } else if let Some((dline, dts)) = &detected_track_line {
                        if dts.elapsed() <= Duration::from_secs(5) {
                            format!("status: {}", dline)
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };
                    let msg = compose_two_line_osd(&status_line, &detail);
                    if msg != last_overlay_msg || last_overlay_push.elapsed() >= Duration::from_secs(2) {
                        push_live_overlay(cfg, &msg, 2)?;
                        last_overlay_msg = msg;
                        last_overlay_push = Instant::now();
                    }
                    eprintln!(
                        "best={} sel={} score={:.2} dist={:.2}m coarse={:.0}%",
                        best.name,
                        selected.name,
                        selected.final_score,
                        selected.mean_dist_m,
                        selected.coarse_inlier_ratio * 100.0
                    );
                    if !best.coarse_pass {
                        if let Some(catalog) = pacenote_stage_catalog.as_ref() {
                            if catalog.len() > 0
                                && last_pacenote_anchor_help.elapsed()
                                    >= Duration::from_secs(PACENOTE_ANCHOR_HELP_SECS)
                            {
                                last_pacenote_anchor_help = Instant::now();
                                eprintln!(
                                    "pacenote anchor hint: history_len={} query_len={} best_coarse={:.0}%",
                                    history.len(),
                                    query.len(),
                                    best.coarse_inlier_ratio * 100.0,
                                );
                                let rows = catalog.distances_to_first_anchors_sorted(
                                    p.x,
                                    p.z,
                                    Some(&ref_names_for_pacenotes),
                                );
                                if rows.is_empty() {
                                    eprintln!(
                                        "pacenote anchor hint: no GeoJSON stages reference loaded refs ({})",
                                        ref_names_for_pacenotes
                                            .iter()
                                            .cloned()
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    );
                                } else {
                                    eprintln!(
                                        "pacenote anchor hint (coarse match failed): distance to 1st callout per stage [loaded refs only], nearest first:"
                                    );
                                    const MAX: usize = 14;
                                    for (d, slug, ref_t) in rows.iter().take(MAX) {
                                        eprintln!("  {:.0} m  {}  (ref {})", d, slug, ref_t);
                                    }
                                    if rows.len() > MAX {
                                        eprintln!("  ... {} more", rows.len() - MAX);
                                    }
                                }
                            }
                        }
                    }
                }
                last_eval = Instant::now();
            }
            if let Some(ref st_amb) = start_track_ambiguous_pick {
                let msg = pacenote_amb_overlay::build_track_start_pick_overlay_text(st_amb);
                push_live_overlay(cfg, &msg, pacenote_amb_overlay::OVERLAY_MAX_LINES)?;
                last_overlay_msg = msg;
                last_overlay_push = Instant::now();
            } else if let Some(ref amb) = pacenote_ambiguous_pick {
                let msg = pacenote_amb_overlay::build_overlay_text(amb);
                push_live_overlay(cfg, &msg, pacenote_amb_overlay::OVERLAY_MAX_LINES)?;
                last_overlay_msg = msg;
                last_overlay_push = Instant::now();
            }
        } else {
            if no_data_since.is_none() {
                no_data_since = Some(Instant::now());
            }
            if !have_physics_frame && last_no_data_log.elapsed() >= Duration::from_secs(3) {
                eprintln!(
                    "still waiting for first ACC physics frame (shared memory maps are open; start ACC / enter driving if idle)..."
                );
                last_no_data_log = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    if let Ok(n) = acr_timing::timing_db::promote_all_pending(&timing_conn) {
        if n > 0 {
            eprintln!("promoted {} pending split(s) on shutdown", n);
        }
    }
    Ok(())
}

fn load_start_points_index(path: &Path) -> Result<HashMap<String, Vec<Point2>>, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let raw = fs::read_to_string(path)?;
    let root: serde_json::Value = serde_json::from_str(&raw)?;
    let mut out: HashMap<String, Vec<Point2>> = HashMap::new();
    let Some(features) = root.get("features").and_then(|v| v.as_array()) else {
        return Ok(out);
    };
    for f in features {
        let track = f
            .get("properties")
            .and_then(|p| p.get("track"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if track.is_empty() {
            continue;
        }
        let Some(coords) = f
            .get("geometry")
            .and_then(|g| g.get("coordinates"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        if coords.len() < 2 {
            continue;
        }
        let Some(file_x) = coords[0].as_f64() else { continue };
        let Some(file_y) = coords[1].as_f64() else { continue };
        let (x, z) = acr_telemetry::gis::file_to_game_xz(file_x, file_y);
        out.entry(track).or_default().push(Point2 { x, z });
    }
    Ok(out)
}

fn select_track_from_starts(
    idx: &HashMap<String, Vec<Point2>>,
    p: Point2,
    radius_m: f64,
) -> Option<String> {
    if idx.is_empty() {
        return None;
    }
    let mut hits: Vec<String> = Vec::new();
    for (track, pts) in idx {
        if pts.iter().any(|sp| dist(*sp, p) <= radius_m) {
            hits.push(track.clone());
        }
    }
    if hits.len() == 1 {
        Some(hits.remove(0))
    } else {
        None
    }
}

/// Reference names that have a `start_points.geojson` anchor within `radius_m` of `p`, limited to loaded `refs`.
fn tracks_within_start_points(
    idx: &HashMap<String, Vec<Point2>>,
    p: Point2,
    radius_m: f64,
    refs: &[ReferenceTrack],
) -> Vec<String> {
    if idx.is_empty() {
        return Vec::new();
    }
    let ref_ok: HashSet<&str> = refs.iter().map(|r| r.name.as_str()).collect();
    let mut hits: Vec<String> = Vec::new();
    for (track, pts) in idx {
        if !ref_ok.contains(track.as_str()) {
            continue;
        }
        if pts.iter().any(|sp| dist(*sp, p) <= radius_m) {
            hits.push(track.clone());
        }
    }
    hits.sort();
    hits
}

fn persist_split_and_line(
    conn: &rusqlite::Connection,
    split: &acr_timing::timing_db::SplitRecord<'_>,
) -> (String, f64) {
    // Compare against the best time that existed *before* inserting this split.
    // Otherwise, any new PB would always show delta 0.000 by definition.
    let best_before = acr_timing::timing_db::best_time(
        conn,
        split.track_name,
        split.car_model,
        split.direction,
        split.from_sector,
        split.to_sector,
    )
    .ok()
    .flatten();
    let _ = acr_timing::timing_db::insert_split(conn, split);
    let delta = best_before
        .map(|b| split.duration_sec - b)
        .unwrap_or(0.0);
    let sign = if delta >= 0.0 { "+" } else { "-" };
    let from_label = if split.from_sector == START_SECTOR_ID {
        "Start".to_string()
    } else {
        split.from_sector.to_string()
    };
    let to_label = if split.to_sector == FINISH_SECTOR_ID {
        "Finish".to_string()
    } else {
        split.to_sector.to_string()
    };
    let prefix = if split.to_sector == FINISH_SECTOR_ID {
        "overall"
    } else {
        "sector"
    };
    let line = format!(
        "{prefix} [{from_label}]-[{to_label}]: {:.3}s ({}{:0.3}s)",
        split.duration_sec,
        sign,
        delta.abs()
    );
    (line, delta)
}

fn field_value_to_string(v: Option<&FieldValue>) -> Option<String> {
    match v? {
        FieldValue::Character(Some(s)) => Some(s.trim().to_string()),
        FieldValue::Numeric(Some(n)) => Some(format!("{n:.0}")),
        FieldValue::Float(Some(f)) => Some(format!("{f:.0}")),
        FieldValue::Integer(i) => Some(i.to_string()),
        FieldValue::Double(d) => Some(format!("{d:.0}")),
        FieldValue::Logical(Some(b)) => Some(if *b { "1".into() } else { "0".into() }),
        _ => None,
    }
}

fn field_value_to_i32(v: Option<&FieldValue>) -> Option<i32> {
    match v? {
        FieldValue::Numeric(Some(n)) => Some(*n as i32),
        FieldValue::Float(Some(f)) => Some(*f as i32),
        FieldValue::Integer(i) => Some(*i),
        FieldValue::Double(d) => Some(*d as i32),
        FieldValue::Character(Some(s)) => s.trim().parse::<i32>().ok(),
        _ => None,
    }
}

fn normalize_track_key(s: &str) -> String {
    s.trim().to_lowercase().replace(' ', "_")
}

fn load_sector_sets_from_shp(
    shp_path: &Path,
    coord_space: SectorsCoordSpace,
    track_field: &str,
    sector_id_field: &str,
    refs: &[ReferenceTrack],
) -> Result<HashMap<String, SectorSet>, Box<dyn std::error::Error>> {
    let mut grouped: HashMap<String, Vec<SectorBoundary>> = HashMap::new();
    let mut reader = shapefile::Reader::from_path(shp_path)?;
    for item in reader.iter_shapes_and_records() {
        let (shape, rec) = item?;
        let track_name = field_value_to_string(rec.get(track_field))
            .ok_or_else(|| format!("Missing or invalid '{track_field}' in sectors SHP"))?;
        let sector_id = field_value_to_i32(rec.get(sector_id_field))
            .ok_or_else(|| format!("Missing or invalid '{sector_id_field}' in sectors SHP"))?;
        let (a, b) = match shape {
            shapefile::Shape::Polyline(pl) => {
                let first_part = pl.parts().first();
                if let Some(part) = first_part {
                    if part.len() < 2 {
                        continue;
                    }
                    let pa = part.first().unwrap();
                    let pb = part.last().unwrap();
                    (
                        point2_from_sectors_shp(pa.x, pa.y, coord_space),
                        point2_from_sectors_shp(pb.x, pb.y, coord_space),
                    )
                } else {
                    continue;
                }
            }
            shapefile::Shape::PolylineM(pl) => {
                let first_part = pl.parts().first();
                if let Some(part) = first_part {
                    if part.len() < 2 {
                        continue;
                    }
                    let pa = part.first().unwrap();
                    let pb = part.last().unwrap();
                    (
                        point2_from_sectors_shp(pa.x, pa.y, coord_space),
                        point2_from_sectors_shp(pb.x, pb.y, coord_space),
                    )
                } else {
                    continue;
                }
            }
            shapefile::Shape::PolylineZ(pl) => {
                let first_part = pl.parts().first();
                if let Some(part) = first_part {
                    if part.len() < 2 {
                        continue;
                    }
                    let pa = part.first().unwrap();
                    let pb = part.last().unwrap();
                    (
                        point2_from_sectors_shp(pa.x, pa.y, coord_space),
                        point2_from_sectors_shp(pb.x, pb.y, coord_space),
                    )
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        grouped
            .entry(normalize_track_key(&track_name))
            .or_default()
            .push(SectorBoundary { sector_id, a, b });
    }

    let mut out = HashMap::new();
    for r in refs {
        let k = normalize_track_key(&r.name);
        if let Some(bounds) = grouped.get(&k) {
            let mut ids: Vec<i32> = bounds.iter().map(|b| b.sector_id).collect();
            ids.sort();
            ids.dedup();
            let id_to_index = ids
                .iter()
                .enumerate()
                .map(|(i, v)| (*v, i))
                .collect::<HashMap<_, _>>();
            let mut boundaries = Vec::new();
            for b in bounds {
                if let Some(idx) = id_to_index.get(&b.sector_id) {
                    boundaries.push(SectorBoundary {
                        sector_id: *idx as i32,
                        a: b.a,
                        b: b.b,
                    });
                }
            }
            out.insert(
                r.name.clone(),
                SectorSet {
                    boundaries,
                    ring_ids: ids,
                },
            );
        }
    }

    if out.is_empty() {
        eprintln!(
            "No matching sector boundaries loaded from {} (track field='{}').",
            shp_path.display(),
            track_field
        );
    } else {
        eprintln!(
            "Loaded sector boundaries for {} detected tracks from {}",
            out.len(),
            shp_path.display()
        );
    }
    Ok(out)
}

fn first_crossed_sector(
    p0: Point2,
    p1: Point2,
    boundaries: &[SectorBoundary],
    search_radius_m: f64,
) -> Option<(usize, f64)> {
    let mut best: Option<(usize, f64)> = None;
    for b in boundaries {
        let center = Point2 {
            x: (b.a.x + b.b.x) * 0.5,
            z: (b.a.z + b.b.z) * 0.5,
        };
        let d0 = dist(p0, center);
        let d1 = dist(p1, center);
        if d0 > search_radius_m && d1 > search_radius_m {
            continue;
        }
        if let Some(t) = segment_intersection_t(p0, p1, b.a, b.b) {
            let idx = b.sector_id as usize;
            if best.map_or(true, |(_, bt)| t < bt) {
                best = Some((idx, t));
            }
        }
    }
    best
}

fn segment_intersection_t(p0: Point2, p1: Point2, q0: Point2, q1: Point2) -> Option<f64> {
    let r = Point2 {
        x: p1.x - p0.x,
        z: p1.z - p0.z,
    };
    let s = Point2 {
        x: q1.x - q0.x,
        z: q1.z - q0.z,
    };
    let rxs = r.x * s.z - r.z * s.x;
    if rxs.abs() < 1e-9 {
        return None;
    }
    let qp = Point2 {
        x: q0.x - p0.x,
        z: q0.z - p0.z,
    };
    let t = (qp.x * s.z - qp.z * s.x) / rxs;
    let u = (qp.x * r.z - qp.z * r.x) / rxs;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
        Some(t)
    } else {
        None
    }
}

fn match_tracks(query: &[Point2], refs: &[ReferenceTrack], cfg: &CliConfig) -> Vec<MatchScore> {
    let query_headings = compute_headings(query);
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        let mut inliers = 0usize;
        let mut d_sum = 0.0f64;
        let mut h_sum = 0.0f64;
        let mut n = 0usize;
        for i in (0..query.len()).step_by(2) {
            let (nearest_idx, d) = nearest_point_idx(query[i], &r.points);
            if d <= cfg.coarse_buffer_m {
                inliers += 1;
            }
            d_sum += d;
            let hd = angle_diff(query_headings[i], r.headings[nearest_idx]).abs();
            h_sum += hd;
            n += 1;
        }
        let coarse_ratio = if n == 0 { 0.0 } else { inliers as f64 / n as f64 };
        let coarse_pass = coarse_ratio >= cfg.coarse_required_ratio;
        let mean_dist = if n == 0 { f64::INFINITY } else { d_sum / n as f64 };
        let mean_heading = if n == 0 { f64::INFINITY } else { h_sum / n as f64 };
        let coarse_penalty = if coarse_pass { 0.0 } else { 10_000.0 };
        let final_score = mean_dist + (mean_heading * 25.0) + coarse_penalty;
        out.push(MatchScore {
            name: r.name.clone(),
            coarse_pass,
            coarse_inlier_ratio: coarse_ratio,
            mean_dist_m: mean_dist,
            mean_heading_diff_rad: mean_heading,
            final_score,
        });
    }
    out.sort_by(|a, b| a.final_score.partial_cmp(&b.final_score).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn print_scores(scores: &[MatchScore]) {
    eprintln!("Track matching results (best first):");
    for s in scores {
        eprintln!(
            "  {:<24} score={:8.3} dist={:6.2}m heading={:5.3}rad coarse={:.0}% {}",
            s.name,
            s.final_score,
            s.mean_dist_m,
            s.mean_heading_diff_rad,
            s.coarse_inlier_ratio * 100.0,
            if s.coarse_pass { "PASS" } else { "FAIL" }
        );
    }
}

fn nearest_point_idx(p: Point2, pts: &[Point2]) -> (usize, f64) {
    let mut best_i = 0usize;
    let mut best_d = f64::INFINITY;
    for (i, rp) in pts.iter().enumerate() {
        let d = dist(p, *rp);
        if d < best_d {
            best_d = d;
            best_i = i;
        }
    }
    (best_i, best_d)
}

fn dist(a: Point2, b: Point2) -> f64 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    (dx * dx + dz * dz).sqrt()
}

fn angle_diff(a: f64, b: f64) -> f64 {
    let mut d = a - b;
    while d > std::f64::consts::PI {
        d -= 2.0 * std::f64::consts::PI;
    }
    while d < -std::f64::consts::PI {
        d += 2.0 * std::f64::consts::PI;
    }
    d
}

fn ctrlc_handler() {
    ctrlc::set_handler(|| {
        RUNNING.store(false, Ordering::Relaxed);
    })
    .expect("could not set Ctrl+C handler");
}

fn load_labels(cfg: &CliConfig) -> Result<std::collections::HashMap<String, String>, Box<dyn std::error::Error>> {
    if let Some(path) = &cfg.labels_path {
        return parse_labels_file(path);
    }
    // Auto-detect labels file in any reference directory.
    for r in &cfg.refs {
        if r.is_dir() {
            let p = r.join("track_labels.toml");
            if p.exists() {
                return parse_labels_file(&p);
            }
        }
    }
    Ok(std::collections::HashMap::new())
}

fn parse_labels_file(path: &Path) -> Result<std::collections::HashMap<String, String>, Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string(path)?;
    let parsed: TrackLabelsFile = toml::from_str(&raw)?;
    Ok(parsed.labels)
}

fn apply_pacenote_first_anchor_resolution(
    pick: &PacenoteStagePick,
    active_pacenote_stage_path: &mut Option<PathBuf>,
    pacenote_cfg: &PacenoteConfig,
    locked_track: Option<&str>,
    active_track_name: Option<&str>,
    pacenote_course: &mut Option<PacenoteCourse>,
    pacenote_course_track: &mut Option<String>,
    triggered_pacenotes: &mut HashSet<usize>,
    last_pacenote_gear_eval: &mut Instant,
    pacenote_gear_extra_lead_sec: &mut f64,
    manual_anchor_slug: &mut Option<String>,
    loaded_src_mtime: &mut Option<SystemTime>,
) {
    let same_path = active_pacenote_stage_path.as_ref() == Some(&pick.path);
    if same_path {
        if let Ok(mtime) = fs::metadata(&pick.path).and_then(|m| m.modified()) {
            if *loaded_src_mtime == Some(mtime) {
                return;
            }
        }
    }
    clear_pacenote_live(
        pacenote_course,
        pacenote_course_track,
        active_pacenote_stage_path,
        triggered_pacenotes,
        last_pacenote_gear_eval,
        pacenote_gear_extra_lead_sec,
        manual_anchor_slug,
        loaded_src_mtime,
        false,
    );
    *active_pacenote_stage_path = Some(pick.path.clone());
    let track_for_attach = locked_track
        .or(active_track_name)
        .filter(|t| *t == pick.reference_track.as_str());
    let pacenote_sel = active_pacenote_stage_path.clone();
    if let Some(tn) = track_for_attach {
        attach_pacenote_course_for_track(
            pacenote_cfg,
            tn,
            pacenote_sel.as_deref(),
            active_pacenote_stage_path,
            None,
            None,
            pacenote_course,
            pacenote_course_track,
            triggered_pacenotes,
            loaded_src_mtime,
        );
    }
    eprintln!(
        "pacenote stage from first-anchor: {} (ref {})",
        pick.slug, pick.reference_track
    );
}

/// Write overlay text atomically: temp file in same directory, then replace target.
/// Avoids readers (e.g. RTSS) seeing a half-written file on Windows.
fn write_overlay_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("acr_detected_track.txt");
    let tmp = dir.join(format!("{}.tmp", name));
    std::fs::write(&tmp, contents)?;
    // On Windows, rename does not replace an existing destination.
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn push_live_overlay(
    cfg: &CliConfig,
    msg: &str,
    rtss_max_lines: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = write_overlay_atomic(&cfg.overlay_file, msg);
    #[cfg(windows)]
    {
        if cfg.rtss {
            let safe = acr_timing::rtss_osd::sanitize_multiline_osd_text(msg, rtss_max_lines);
            if let Err(e) = acr_timing::rtss_osd::update(&cfg.rtss_owner, &safe, cfg.rtss_slot) {
                eprintln!("RTSS update failed: {}", e);
            }
        }
    }
    Ok(())
}

fn compose_two_line_osd(status: &str, detail: &str) -> String {
    let status = status.trim();
    let detail = detail.trim();
    if detail.is_empty() {
        format!("{}\n", status)
    } else {
        format!("{}\n{}", status, detail)
    }
}

fn append_start_point_geojson(
    path: &Path,
    track_name: &str,
    car_model: &str,
    p: Point2,
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut root = if path.exists() {
        let raw = fs::read_to_string(path)?;
        serde_json::from_str::<serde_json::Value>(&raw).unwrap_or_else(|_| {
            serde_json::json!({
                "type": "FeatureCollection",
                "features": []
            })
        })
    } else {
        serde_json::json!({
            "type": "FeatureCollection",
            "features": []
        })
    };

    if root.get("type").and_then(|v| v.as_str()) != Some("FeatureCollection") {
        root = serde_json::json!({
            "type": "FeatureCollection",
            "features": []
        });
    }

    let (file_x, file_y) = acr_telemetry::gis::game_xz_to_file(p.x, p.z);
    let feature = serde_json::json!({
        "type": "Feature",
        "geometry": {
            "type": "Point",
            "coordinates": [file_x, file_y]
        },
        "properties": {
            "kind": "start_anchor",
            "track": track_name,
            "car": car_model,
            "ts_utc": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        }
    });

    if let Some(arr) = root.get_mut("features").and_then(|v| v.as_array_mut()) {
        arr.push(feature);
    } else {
        root["features"] = serde_json::json!([feature]);
    }
    fs::write(path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

fn clear_pacenote_live(
    course: &mut Option<PacenoteCourse>,
    course_track: &mut Option<String>,
    stage_path: &mut Option<PathBuf>,
    triggered: &mut HashSet<usize>,
    last_gear_eval: &mut Instant,
    gear_extra_lead_sec: &mut f64,
    manual_anchor_slug: &mut Option<String>,
    loaded_src_mtime: &mut Option<SystemTime>,
    clear_manual_anchor_slug: bool,
) {
    *course = None;
    *course_track = None;
    *stage_path = None;
    *loaded_src_mtime = None;
    triggered.clear();
    *gear_extra_lead_sec = 0.0;
    *last_gear_eval = Instant::now();
    if clear_manual_anchor_slug {
        *manual_anchor_slug = None;
    }
}

fn attach_pacenote_course_for_track(
    cfg: &PacenoteConfig,
    track_name: &str,
    selected_stage_path: Option<&Path>,
    active_pacenote_stage_path: &mut Option<PathBuf>,
    player_xz: Option<(f64, f64)>,
    pacenote_stage_catalog: Option<&PacenoteStageCatalog>,
    course: &mut Option<PacenoteCourse>,
    course_track: &mut Option<String>,
    triggered: &mut HashSet<usize>,
    loaded_src_mtime: &mut Option<SystemTime>,
) {
    let path = if let Some(path) = selected_stage_path {
        if path.is_file() {
            Some(path.to_path_buf())
        } else {
            eprintln!("pacenotes geojson not found: {}", path.display());
            None
        }
    } else if let Some(path) = &cfg.geojson {
        if path.is_file() {
            Some(path.clone())
        } else {
            eprintln!("pacenotes geojson not found: {}", path.display());
            None
        }
    } else if let Some(dir) = &cfg.pacenotes_dir {
        let cand = if cfg.ref_geojson_candidates.is_empty() {
            None
        } else {
            Some(&cfg.ref_geojson_candidates)
        };
        pacenote_course::pick_geojson_for_locked_reference(
            dir,
            track_name,
            cfg.stage.as_deref(),
            cand,
            player_xz,
            pacenote_stage_catalog,
            cfg.first_anchor_lock_radius_m,
        )
    } else {
        None
    };
    let Some(path) = path else {
        if cfg.geojson.is_none() {
            eprintln!(
                "pacenotes: no stage/geojson match for track '{}'; set [pacenotes].ref_geojson_candidates, or stage/geojson, or a <track>.geojson under pacenotes_dir",
                track_name
            );
        }
        return;
    };
    if course_track.as_deref() == Some(track_name)
        && course.is_some()
        && active_pacenote_stage_path.as_deref() == Some(path.as_path())
    {
        if let Ok(mtime) = fs::metadata(&path).and_then(|m| m.modified()) {
            if *loaded_src_mtime == Some(mtime) {
                return;
            }
        }
    }
    *course = None;
    *course_track = None;
    triggered.clear();
    match pacenote_course::load_course(&path) {
        Ok(loaded) => {
            eprintln!(
                "pacenotes loaded: {} ({} callouts) from {}",
                loaded.stage,
                loaded.callouts.len(),
                path.display()
            );
            *course = Some(loaded);
            *course_track = Some(track_name.to_string());
            *loaded_src_mtime = fs::metadata(&path).and_then(|m| m.modified()).ok();
            *active_pacenote_stage_path = Some(path);
        }
        Err(e) => eprintln!("pacenotes load failed ({}): {}", path.display(), e),
    }
}

fn collect_callout_tokens(course: &PacenoteCourse, start: usize) -> (Vec<String>, Vec<usize>) {
    let mut tokens = Vec::new();
    let mut indices = Vec::new();
    let mut pos = start;
    loop {
        let callout = &course.callouts[pos];
        indices.push(callout.index);
        tokens.extend(callout.notes.iter().cloned());
        if callout.link_to_next && pos + 1 < course.callouts.len() {
            pos += 1;
        } else {
            break;
        }
    }
    (tokens, indices)
}

fn log_pacenote_enqueue_lead(
    callout: &PacenoteCallout,
    speed_kmh: f64,
    acc_gear: i32,
    cfg: &PacenoteConfig,
    lead_sec_base: f64,
    slow_corner_extra: f64,
    conflict_advance_sec: f64,
    gear_extra_lead_sec: f64,
    lookahead_m: f64,
    lookahead_uncapped_m: f64,
    leg_to_next_m: f64,
    distance_m: f64,
    ahead: bool,
    passed: bool,
    callout_urgency: u8,
    next_urgency: Option<u8>,
    time_to_next_callout_sec: f64,
) {
    let total_lead_sec =
        lead_sec_base + slow_corner_extra + conflict_advance_sec + gear_extra_lead_sec;
    let trigger_mode = if passed {
        "radius"
    } else if ahead {
        "ahead"
    } else {
        "unknown"
    };
    let next_urgency = next_urgency
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let corner_gear = callout
        .max_turn_severity
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    eprintln!(
        "pacenote lead [{}] {} | corner_gear={} urgency={} next_urgency={} | v={:.1} km/h acc_gear={} | lead_sec base={:.3} slow={:.3} conflict={:.3} gear={:.3} total={:.3} | dist_m route={:.1} lookahead={:.1} raw={:.1} leg_next={:.1} skip_buf={:.1} | t_next={:.3}s trigger={}",
        callout.index,
        callout.notes_text,
        corner_gear,
        callout_urgency,
        next_urgency,
        speed_kmh,
        acc_gear,
        lead_sec_base,
        slow_corner_extra,
        conflict_advance_sec,
        gear_extra_lead_sec,
        total_lead_sec,
        distance_m,
        lookahead_m,
        lookahead_uncapped_m,
        leg_to_next_m,
        cfg.skip_buffer_m,
        time_to_next_callout_sec,
        trigger_mode,
    );
}

