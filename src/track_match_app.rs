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
use acr_timing::cumulative_sector_timing::CumulativeTrackSectors;
use acr_timing::sector_leg_stats::{SectorLegStatsAccumulator, SectorLegStatsSnapshot};
use acr_timing::split_beep::SplitBeepConfig;
use acr_timing_engine::{RunCoordinator, SectorSessionConfig, sector_boundaries_from_labels};
use acr_timing_presenter::PresenterState;
use acr_timing_protocol::{EventReceiver, EventSender, TimingEventBody};
use acr_timing_store::ReferenceStore;
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
/// In-game restart: reset an active run after this long at a known `start_points` anchor.
const START_GRID_TIMING_RESET_STILL_SEC: f64 = 3.0;
/// Clear a pending teleport unlock if the car keeps driving above standstill speed this long.
const TELEPORT_PENDING_CLEAR_DRIVE_SEC: f64 = 2.0;
/// Ignore sector line crossings for a single physics step longer than this (stale `last_pt` / menu teleport).
const MAX_SECTOR_CROSS_SEGMENT_M: f64 = 120.0;
/// RTSS flash after a cumulative (silent) gate: leg Δ vs PB.
const CUMULATIVE_RTSS_FLASH_SEC: u64 = 4;
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

struct LiveTimingState {
    tracker: SectorPassTracker,
    ring_ids: Vec<i32>,
    /// Leg/run timing from physics `packet_id` (see `run_timing_clock`).
    run_clock: acr_timing::run_timing_clock::RunTimingClock,
    last_sector_idx: Option<usize>,
    start_stage_pos: Option<Point2>,
    start_stage_since: Option<Instant>,
    start_stage_last_report_sec: i32,
    start_armed: bool,
    cooldown_until: HashMap<usize, Instant>,
    /// Pacenote-derived start/finish for Gesamtzeit (`timing/overall_markers/<slug>.geojson`).
    overall_markers: Option<acr_timing::stage_overall_markers::StageOverallMarkers>,
    overall_finish_recorded: bool,
    /// Parallel calibrated stage timers (same ref track / spline; up to 3 slugs).
    stage_sector_sessions: Vec<acr_timing::stage_sector_timing::StageSectorSession>,
    /// Physics aggregates for the current subsection leg (anchor → next cross).
    leg_stats: SectorLegStatsAccumulator,
    /// Speed (km/h) at subsection leg entry (set with anchor).
    leg_entry_speed_kmh: Option<f32>,
    /// Stall wall excess since current subsection anchor (SHP sector splits).
    subsection_leg_excess_wall_sec: f64,
    /// Large position jump during current subsection leg.
    subsection_timing_position_reset: bool,
    /// Legs recorded this run (from, to, dt) for cumulative pace on silent splits.
    subsection_run_legs: Vec<(i32, i32, f64)>,
    subsection_cumulative_sec: f64,
    subsection_html_path: Option<PathBuf>,
    subsection_html_run_index: usize,
    /// GeoJSON cumulative gates (replaces SHP subsection timing for this track).
    cumulative: Option<acr_timing::cumulative_sector_timing::CumulativeLegState>,
    /// Event-driven sector/sub timing (reference store + presenter OSD).
    #[allow(dead_code)]
    modular: Option<ModularTimingState>,
}

struct ModularTimingState {
    coordinator: RunCoordinator,
    event_rx: EventReceiver,
    presenter: PresenterState,
}

fn cumulative_ordered_labels(cum: &CumulativeTrackSectors) -> Vec<(i32, String)> {
    cum.sectors
        .markers
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let id = cum.seg_ids.get(i).copied().unwrap_or(m.order);
            (id, m.label.clone())
        })
        .collect()
}

fn ensure_modular_timing(
    state: &mut LiveTimingState,
    bus: &EventSender,
    store_path: &Path,
    cum: &CumulativeTrackSectors,
    reference_track: &str,
    car: &str,
    reference_mode: acr_timing_store::ReferenceTimeMode,
) {
    if state.modular.is_some() {
        return;
    }
    let Ok(store) = ReferenceStore::open(store_path) else {
        eprintln!(
            "timing_reference_store: could not open {}",
            store_path.display()
        );
        return;
    };
    let cfg = SectorSessionConfig {
        reference_track: reference_track.to_string(),
        car: car.to_string(),
        stage_slug: cum.slug.clone(),
    };
    let mut coordinator = RunCoordinator::new(bus.clone(), store, cfg, reference_mode);
    let labels = cumulative_ordered_labels(cum);
    coordinator.set_route(&labels);
    let n_sectors = sector_boundaries_from_labels(&labels).len();
    eprintln!(
        "modular timing: {} / {} ({} main-sector blocks)",
        reference_track, cum.slug, n_sectors
    );
    state.modular = Some(ModularTimingState {
        coordinator,
        event_rx: bus.subscribe(),
        presenter: PresenterState::default(),
    });
}

/// Fresh modular run (clears stale demo / prior-sector OSD) and publishes `TimingStarted`.
fn arm_modular_timing_run(
    state: &mut LiveTimingState,
    cfg: &CliConfig,
    car: &str,
) {
    let Some(m) = state.modular.as_mut() else {
        return;
    };
    m.presenter = PresenterState::default();
    m.coordinator.reset_run();
    m.coordinator.set_car(car);
    m.coordinator.timing_started();
    drain_modular_timing_events(state, cfg, None, None);
}

/// Calibrated stage-sector leg time for main sector `sector_index` (S1 → leg 0, …).
fn stage_tot_sec_for_sector(timing_state: &LiveTimingState, sector_index: u32) -> Option<f64> {
    let leg_ix = sector_index as usize;
    timing_state
        .stage_sector_sessions
        .iter()
        .filter_map(|sess| sess.run.sector_secs.get(leg_ix).and_then(|t| *t))
        .find(|t| t.is_finite() && *t > 0.05)
}

fn stage_sektoren_summe_sec(timing_state: &LiveTimingState) -> f64 {
    timing_state
        .stage_sector_sessions
        .iter()
        .flat_map(|sess| sess.run.sector_secs.iter())
        .filter_map(|t| *t)
        .filter(|t| t.is_finite() && *t > 0.05)
        .sum()
}

#[allow(dead_code)]
struct TimingDebugFrame<'a> {
    physics: &'a PhysicsMap,
    graphics_x: f64,
    graphics_z: f64,
    graphics_current_time_ms: i32,
    speed_kmh: f32,
    distance_traveled_m: f64,
    packet_id: i32,
}

impl TimingDebugFrame<'_> {
    fn spielzeit_sec(&self) -> f64 {
        acr_timing::timing_debug::spielzeit_sec(self.graphics_current_time_ms)
    }

    fn run_sim_sec(&self, state: &LiveTimingState) -> Option<f64> {
        state.run_clock.run_sim_sec(self.packet_id)
    }
}

fn drain_modular_timing_events(
    timing_state: &mut LiveTimingState,
    cfg: &CliConfig,
    debug: Option<&TimingDebugFrame<'_>>,
    sync_brackets: Option<(&acr_timing::timing_pb::TimingPbStore, &str)>,
) {
    let events = match timing_state.modular.as_mut() {
        None => return,
        Some(m) => m.event_rx.drain(),
    };
    for mut event in events {
        if cfg.beep_on_cumulative_split {
            if let TimingEventBody::SubSplit(ref s) = event.body {
                let delta = match cfg.delta_display.split_feedback() {
                    acr_timing::SplitFeedbackDeltaSource::Subsector => {
                        s.delta_i_sec.unwrap_or(s.cum_delta_sec)
                    }
                    acr_timing::SplitFeedbackDeltaSource::Sector => s.cum_delta_sec,
                    acr_timing::SplitFeedbackDeltaSource::Stage => s.cum_delta_sec,
                };
                acr_timing::split_beep::play_split_feedback(delta, &cfg.cumulative_beep);
            }
        }
        if let TimingEventBody::SectorCompleted(ref mut s) = event.body {
            let modular_tot = s.tot_sec;
            let sub_block_sum: f64 = s.sub_times_sec.iter().filter_map(|t| *t).sum();
            if let Some(stage_t) = stage_tot_sec_for_sector(timing_state, s.sector_index) {
                if (modular_tot - stage_t).abs() > 0.05 {
                    eprintln!(
                        "modular: S{} tot {modular_tot:.3}s → stage {stage_t:.3}s (OSD uses stage)",
                        s.sector_index + 1,
                    );
                }
                s.tot_sec = stage_t;
            }
            if cfg.timing_debug {
                if let Some(dbg) = debug {
                    let osd_after = timing_state
                        .modular
                        .as_ref()
                        .map(|m| m.presenter.track_completed_cum_sec() + s.tot_sec)
                        .unwrap_or(s.tot_sec);
                    acr_timing::timing_debug::log_sektor_fertig_vergleich(
                        s.sector_index,
                        s.tot_sec,
                        sub_block_sum,
                        stage_tot_sec_for_sector(timing_state, s.sector_index),
                        timing_state.subsection_cumulative_sec,
                        osd_after,
                        dbg.run_sim_sec(timing_state),
                        dbg.spielzeit_sec(),
                    );
                }
            }
        }
        if let Some(m) = timing_state.modular.as_mut() {
            m.presenter.apply(&event);
        }
        if let Some((pb, car)) = sync_brackets {
            if matches!(
                event.body,
                TimingEventBody::SectorCompleted(_) | TimingEventBody::SectorStarted(_)
            ) {
                let reset_live = matches!(event.body, TimingEventBody::SectorCompleted(_));
                sync_modular_stage_cum_delta_from_brackets(
                    timing_state,
                    pb,
                    car,
                    cfg.delta_display.delta_scope,
                    reset_live,
                );
            }
        }
        if let TimingEventBody::RunFinished(_) = event.body {
            if cfg.timing_debug {
                if let (Some(dbg), Some(m)) = (debug, timing_state.modular.as_ref()) {
                    acr_timing::timing_debug::log_strecke_fertig_vergleich(
                        m.presenter.track_completed_cum_sec(),
                        timing_state.subsection_cumulative_sec,
                        stage_sektoren_summe_sec(timing_state),
                        dbg.run_sim_sec(timing_state),
                        dbg.spielzeit_sec(),
                    );
                }
            }
        }
    }
}

fn modular_presenter_detail(state: &mut LiveTimingState, cfg: &CliConfig) -> String {
    let Some(m) = state.modular.as_mut() else {
        return String::new();
    };
    let lines = m
        .presenter
        .osd_lines(cfg.rtss, &cfg.delta_display, Some(&cfg.osd_templates));
    if lines.is_empty() {
        return String::new();
    }
    lines.join("\n")
}

fn cumulative_osd_detail(state: &mut LiveTimingState, cfg: &CliConfig, _now: Instant) -> String {
    modular_presenter_detail(state, cfg)
}

/// Modular two-line OSD; optional cumulative CP flash as extra line (does not replace presenter).
fn cumulative_osd_detail_with_flash(
    state: &mut LiveTimingState,
    cfg: &CliConfig,
    sector_status_line: &Option<(String, Instant)>,
    now: Instant,
) -> String {
    let modular = cumulative_osd_detail(state, cfg, now);
    if let Some((flash, sts)) = sector_status_line {
        if sts.elapsed() <= osd_detail_ttl_for_state(state) {
            if modular.is_empty() {
                return flash.clone();
            }
            return format!("{modular}\n{flash}");
        }
    }
    modular
}


/// Completed main-sector legs only (fallback when modular timing is not armed).
fn stage_sessions_scope_delta(
    sessions: &[&acr_timing::stage_sector_timing::StageSectorSession],
    pb: &acr_timing::timing_pb::TimingPbStore,
    car_model: &str,
    scope: acr_timing::delta_display::DeltaScope,
) -> Option<f64> {
    let sess = sessions.first()?;
    let car = if car_model.trim().is_empty() {
        "unknown_car"
    } else {
        car_model.trim()
    };
    let refs = if sess.run.references_frozen() {
        sess.run.reference_secs.clone()
    } else {
        acr_timing::stage_sector_timing::reference_sector_secs_from_pb(
            pb,
            &sess.markers.stage_slug,
            car,
            &sess.markers.markers,
        )
    };
    let cur: Vec<f64> = sess
        .run
        .sector_secs
        .iter()
        .filter_map(|t| *t)
        .collect();
    match scope {
        acr_timing::delta_display::DeltaScope::Stage => {
            acr_timing::stage_sector_timing::stage_scope_delta_sec(&cur, &refs)
        }
        acr_timing::delta_display::DeltaScope::Sector => {
            if cur.is_empty() {
                return None;
            }
            let i = cur.len() - 1;
            let r = refs.get(i).copied().flatten()?;
            if !r.is_finite() || r < 0.05 {
                return None;
            }
            Some(cur[i] - r)
        }
        acr_timing::delta_display::DeltaScope::Subsector => None,
    }
}

/// `delta_scope = stage`: set `run_cum_delta_sec` from bracket sum; keep `last_cum_delta_sec` for open sector subs unless reset.
fn sync_modular_stage_cum_delta_from_brackets(
    state: &mut LiveTimingState,
    timing_pb: &acr_timing::timing_pb::TimingPbStore,
    car_model: &str,
    scope: acr_timing::delta_display::DeltaScope,
    reset_live_sub_cum: bool,
) {
    if scope != acr_timing::delta_display::DeltaScope::Stage {
        return;
    }
    let Some(m) = state.modular.as_mut() else {
        return;
    };
    let session_refs: Vec<_> = state.stage_sector_sessions.iter().collect();
    if session_refs.is_empty() {
        return;
    }
    let Some(bracket_sum) = stage_sessions_scope_delta(&session_refs, timing_pb, car_model, scope)
    else {
        return;
    };
    m.presenter
        .sync_stage_cumulative_from_brackets(bracket_sum, reset_live_sub_cum);
}

fn minimal_big_delta_line(
    state: &mut LiveTimingState,
    cfg: &CliConfig,
    stage_delta: Option<f64>,
    pause_dash: bool,
    pause_osd: &mut acr_timing::game_clock_sync::PauseOsdState,
) -> String {
    let style = &cfg.delta_display.colors;
    let scale = cfg.osd_templates.live_delta_font_scale;
    let scope = cfg.delta_display.delta_scope;
    let modular_delta = state
        .modular
        .as_ref()
        .and_then(|m| m.presenter.osd_cumulative_delta_sec(scope));
    let delta_opt = modular_delta.or(stage_delta);

    if pause_dash {
        if pause_osd.frozen_cum_delta_sec.is_none() {
            pause_osd.frozen_cum_delta_sec = delta_opt;
        }
        return acr_timing::minimal_osd::format_minimal_big_delta_opt(
            pause_osd.frozen_cum_delta_sec,
            cfg.rtss,
            style,
            scale,
        );
    }
    pause_osd.frozen_cum_delta_sec = None;

    let Some(m) = state.modular.as_mut() else {
        return acr_timing::minimal_osd::format_minimal_big_delta_opt(
            delta_opt,
            cfg.rtss,
            style,
            scale,
        );
    };
    m.presenter
        .refresh_live(cfg.rtss, style, Some(&cfg.osd_templates));
    acr_timing::minimal_osd::format_minimal_big_delta_opt(delta_opt, cfg.rtss, style, scale)
}

fn build_rtss_pre_lock_message(cfg: &CliConfig) -> String {
    use acr_timing::osd_template::OsdTemplatePreset;

    if !cfg.rtss || cfg.osd_templates.preset != OsdTemplatePreset::Minimal {
        return String::new();
    }
    let game_data_available = cfg.game_clock.enabled
        && acr_timing::minimal_osd::game_clock_timer_ready(
            &cfg.game_clock.jsonl_path,
            cfg.game_clock.max_sample_age_sec,
        );
    acr_timing::minimal_osd::compose_minimal_pre_lock_osd(game_data_available)
}

fn build_rtss_timing_message(
    ts: &mut LiveTimingState,
    cfg: &CliConfig,
    timing_pb: &acr_timing::timing_pb::TimingPbStore,
    car_osd: &str,
    now: Instant,
    game_race_s: Option<f64>,
    pause_dash: bool,
    pause_osd: &mut acr_timing::game_clock_sync::PauseOsdState,
    sector_status_line: &Option<(String, Instant)>,
) -> String {
    use acr_timing::osd_template::OsdTemplatePreset;

    if cfg.osd_templates.preset != OsdTemplatePreset::Minimal {
        let cum_detail = if ts.cumulative.is_some() {
            cumulative_osd_detail_with_flash(ts, cfg, sector_status_line, now)
        } else {
            String::new()
        };
        if !ts.stage_sector_sessions.is_empty() {
            let session_refs: Vec<_> = ts.stage_sector_sessions.iter().collect();
            let strip = acr_timing::stage_sector_timing::format_multi_stage_sector_line(
                &session_refs,
                timing_pb,
                car_osd,
                cfg.rtss,
                now,
                &cfg.delta_display.colors,
            );
            return acr_timing::stage_sector_timing::compose_timing_osd(&strip, &cum_detail);
        }
        return if ts.cumulative.is_some() {
            cum_detail
        } else {
            String::new()
        };
    }

    let pre_start = {
        let session_refs: Vec<_> = ts.stage_sector_sessions.iter().collect();
        !session_refs.is_empty()
            && acr_timing::minimal_osd::stage_sessions_pre_start(&session_refs)
            && acr_timing::minimal_osd::pre_start_from_race_time(game_race_s)
    };

    let mut upper = {
        let session_refs: Vec<_> = ts.stage_sector_sessions.iter().collect();
        if session_refs.is_empty() {
            String::new()
        } else {
            acr_timing::minimal_osd::format_minimal_multi_stage_upper(
                &session_refs,
                timing_pb,
                car_osd,
                now,
                pre_start,
                pause_dash,
                cfg.rtss,
                &cfg.delta_display.colors,
                game_race_s,
            )
        }
    };

    if !pre_start && cfg.game_clock.enabled {
        if let Some((sample, _)) = acr_timing::game_clock_sync::read_latest_sample(
            &cfg.game_clock.jsonl_path,
            cfg.game_clock.max_sample_age_sec,
        ) {
            if let Some(p) = acr_timing::minimal_osd::penalty_from_sample(&sample) {
                upper.push_str(&acr_timing::minimal_osd::format_minimal_penalty_suffix(
                    p, cfg.rtss,
                ));
            }
        }
    }

    let delta_line = if pre_start {
        acr_timing::minimal_osd::format_minimal_pre_start_big_delta(
            cfg.rtss,
            &cfg.delta_display.colors,
            cfg.osd_templates.live_delta_font_scale,
        )
    } else {
        let stage_delta = {
            let session_refs: Vec<_> = ts.stage_sector_sessions.iter().collect();
            if session_refs.is_empty() {
                None
            } else {
                stage_sessions_scope_delta(
                    &session_refs,
                    timing_pb,
                    car_osd,
                    cfg.delta_display.delta_scope,
                )
            }
        };
        minimal_big_delta_line(ts, cfg, stage_delta, pause_dash, pause_osd)
    };

    let mut status = if pre_start {
        let timer_ready = cfg.game_clock.enabled
            && acr_timing::minimal_osd::game_clock_timer_ready(
                &cfg.game_clock.jsonl_path,
                cfg.game_clock.max_sample_age_sec,
            );
        acr_timing::minimal_osd::ready_status_text(timer_ready).to_string()
    } else {
        String::new()
    };

    if !pre_start {
        if let Some((flash, sts)) = sector_status_line {
            if sts.elapsed() <= osd_detail_ttl_for_state(ts) {
                status = flash.clone();
            }
        }
    }

    if upper.is_empty() && delta_line.is_empty() && status.is_empty() {
        if ts.cumulative.is_some() {
            return acr_timing::minimal_osd::compose_minimal_timing_osd("", &delta_line, "");
        }
        return String::new();
    }

    acr_timing::minimal_osd::compose_minimal_timing_osd(&upper, &delta_line, &status)
}

fn osd_detail_ttl_for_state(state: &LiveTimingState) -> Duration {
    if state.cumulative.is_some() {
        Duration::from_secs(CUMULATIVE_RTSS_FLASH_SEC)
    } else {
        Duration::from_secs(8)
    }
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
    if state.run_clock.leg_anchor().is_some() || state.start_armed {
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

/// Attach all calibrated stage sector sets for the locked reference track (parallel timers).
fn ensure_stage_timing_sectors(
    state: &mut LiveTimingState,
    reference_track: &str,
    stage_timing: &acr_timing::stage_timing_config::StageTimingConfig,
    cache: &mut acr_timing::timing_sectors::SectorCache,
    active_stage_slug: &mut Option<String>,
    pos: Option<(f64, f64)>,
    heading_rad: Option<f32>,
) {
    // Cumulative GeoJSON is calibrated for one stage variant; ignore parallel alternates.
    let catalog = if state.cumulative.is_some() {
        stage_timing
            .stage_slug_for_reference(reference_track)
            .into_iter()
            .collect::<Vec<_>>()
    } else {
        stage_timing.stage_slugs_for_reference(reference_track)
    };
    if catalog.is_empty() {
        return;
    }
    let allowed: HashSet<String> = catalog.iter().cloned().collect();
    state
        .stage_sector_sessions
        .retain(|s| allowed.contains(&s.markers.stage_slug));
    let sectors_dir = stage_timing.sectors_dir();
    let start_radius = stage_timing.stage_sector_radius_m();
    let to_attach = acr_timing::timing_sectors::resolve_active_stage_slugs(
        &catalog,
        &sectors_dir,
        cache,
        pos,
        heading_rad,
        start_radius,
    );
    for (slug, shadow_companion) in to_attach {
        if state
            .stage_sector_sessions
            .iter()
            .any(|s| s.markers.stage_slug == slug)
        {
            continue;
        }
        let Some(markers) =
            acr_timing::timing_sectors::load_for_stage_slug(&slug, &sectors_dir, cache)
        else {
            eprintln!(
                "stage timing: no GeoJSON for slug '{}' (expected {}/{}.geojson)",
                slug,
                sectors_dir.display(),
                slug
            );
            continue;
        };
        let sess = acr_timing::stage_sector_timing::StageSectorSession::new_with_attach(
            markers.clone(),
            shadow_companion,
        );
        let mode = if shadow_companion {
            "companion (also_run)"
        } else {
            "primary"
        };
        eprintln!(
            "stage timing [{mode}]: {} → {} [{}] ({} markers, {} legs)",
            sess.markers.stage_slug,
            sess.markers.ziel,
            sess.markers.rtss_label(),
            sess.markers.markers.len(),
            sess.markers.sector_leg_count,
        );
        state.stage_sector_sessions.push(sess);
    }
    if state.stage_sector_sessions.is_empty() {
        return;
    }
    if active_stage_slug.is_none() {
        eprintln!(
            "parallel stage timers: {} active (max {})",
            state.stage_sector_sessions.len(),
            acr_timing::stage_timing_config::MAX_PARALLEL_STAGE_TIMINGS
        );
        *active_stage_slug = Some(state.stage_sector_sessions[0].markers.stage_slug.clone());
    }
}

fn freeze_stage_run_references(
    session: &mut acr_timing::stage_sector_timing::StageSectorSession,
    cfg: &CliConfig,
    pb: &acr_timing::timing_pb::TimingPbStore,
    store: Option<&ReferenceStore>,
    reference_track: &str,
    car_model: &str,
) {
    let car = if car_model.trim().is_empty() {
        "unknown_car"
    } else {
        car_model.trim()
    };
    let refs = acr_timing::stage_sector_timing::snapshot_stage_reference_secs(
        cfg.reference_times.mode,
        pb,
        store,
        reference_track,
        &session.markers.stage_slug,
        car,
        &session.markers.markers,
    );
    let n = refs.iter().filter(|r| r.is_some()).count();
    session.run.freeze_reference_secs(refs);
    eprintln!(
        "timing: frozen {n}/{total} reference legs for {label} (mode={mode})",
        n = n,
        total = session.run.reference_secs.len(),
        label = session.markers.rtss_label(),
        mode = cfg.reference_times.mode.as_str(),
    );
}

fn flush_one_stage_sector_session(
    session: &mut acr_timing::stage_sector_timing::StageSectorSession,
    cfg: &CliConfig,
    timing_pb: &mut acr_timing::timing_pb::TimingPbStore,
    car_model: &str,
    run_counter: &mut usize,
) {
    if !session.run.any_sector() {
        session.run.reset_run();
        return;
    }
    if session.run.references_frozen() {
        match acr_timing::stage_sector_timing::commit_stage_run_to_pb(timing_pb, session, car_model)
        {
            Ok(n) if n > 0 => eprintln!(
                "timing_pb: [{}] run commit updated {n} leg(s)",
                session.markers.rtss_label()
            ),
            Ok(_) => {}
            Err(e) => eprintln!(
                "timing_pb: [{}] run commit failed: {}",
                session.markers.rtss_label(),
                e
            ),
        }
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

fn flush_all_stage_sector_sessions(
    state: &mut LiveTimingState,
    cfg: &CliConfig,
    timing_pb: &mut acr_timing::timing_pb::TimingPbStore,
    car_model: &str,
    run_counter: &mut usize,
) {
    for session in &mut state.stage_sector_sessions {
        flush_one_stage_sector_session(session, cfg, timing_pb, car_model, run_counter);
    }
}

fn note_stage_timing_position_reset(state: &mut LiveTimingState) {
    for session in &mut state.stage_sector_sessions {
        if !session.run.armed || session.run.completed {
            continue;
        }
        if session.run.timing_position_reset {
            continue;
        }
        session.run.note_timing_position_reset();
        eprintln!(
            "[{}] {}",
            session.markers.rtss_label(),
            acr_timing::stage_sector_timing::TIMING_POSITION_RESET_WARNING
        );
    }
}

impl LiveTimingState {
    /// New SHP ring tracker; keep cumulative/modular/presenter when re-locking the same track mid-run.
    fn new_preserving_cumulative(
        ring_ids: Vec<i32>,
        prev: Option<Self>,
        track_name: &str,
    ) -> Self {
        let mut s = Self::new(ring_ids);
        let Some(old) = prev else {
            return s;
        };
        let same_track = old.cumulative.as_ref().is_some_and(|c| {
            acr_timing::stage_timing_config::normalize_track_slug(&c.track.reference_track)
                == acr_timing::stage_timing_config::normalize_track_slug(track_name)
        });
        if !same_track {
            return s;
        }
        s.cumulative = old.cumulative;
        s.modular = old.modular;
        s.run_clock = old.run_clock.clone();
        s.subsection_run_legs = old.subsection_run_legs;
        s.subsection_cumulative_sec = old.subsection_cumulative_sec;
        s.stage_sector_sessions = old.stage_sector_sessions;
        eprintln!(
            "timing: preserved cumulative/modular OSD state across track lock ({track_name})"
        );
        s
    }

    fn new(ring_ids: Vec<i32>) -> Self {
        Self {
            tracker: SectorPassTracker::new(ring_ids.len().max(1)),
            ring_ids,
            run_clock: acr_timing::run_timing_clock::RunTimingClock::new(333.0),
            last_sector_idx: None,
            start_stage_pos: None,
            start_stage_since: None,
            start_stage_last_report_sec: -1,
            start_armed: false,
            cooldown_until: HashMap::new(),
            overall_markers: None,
            overall_finish_recorded: false,
            stage_sector_sessions: Vec::new(),
            leg_stats: SectorLegStatsAccumulator::default(),
            leg_entry_speed_kmh: None,
            subsection_leg_excess_wall_sec: 0.0,
            subsection_timing_position_reset: false,
            subsection_run_legs: Vec::new(),
            subsection_cumulative_sec: 0.0,
            subsection_html_path: None,
            subsection_html_run_index: 1,
            cumulative: None,
            modular: None,
        }
    }

}

fn reset_subsection_run(state: &mut LiveTimingState) {
    if !state.subsection_run_legs.is_empty() {
        state.subsection_html_run_index += 1;
    }
    state.subsection_run_legs.clear();
    state.subsection_cumulative_sec = 0.0;
}

fn subsection_timing_active(state: &LiveTimingState) -> bool {
    state.cumulative.is_some() || state.run_clock.leg_anchor().is_some() || state.start_armed
}

fn timing_anchor_now(
    packet_id: i32,
    distance_traveled_m: f64,
    game_race_sec: Option<f64>,
) -> acr_timing::run_timing_clock::TimingAnchor {
    acr_timing::run_timing_clock::TimingAnchor::new(
        packet_id,
        Instant::now(),
        distance_traveled_m,
        game_race_sec,
    )
}

fn compute_subsection_leg_dt(
    state: &mut LiveTimingState,
    packet_id: i32,
    now: Instant,
    cfg: &acr_timing::timing_frame_quality::TimingQualityConfig,
    game_clock: &acr_timing::game_clock_sync::GameClockCorrector,
) -> Option<(f64, f64)> {
    if game_clock.enabled() {
        if let (Some(hud), Some(anchor)) = (
            game_clock.game_race_for_sector_display(),
            state.run_clock.leg_anchor()?.game_race_sec,
        ) {
            let sim = (hud - anchor).max(0.0);
            if sim > 0.05 {
                let dt = finalize_subsection_split_dt(state, sim, cfg);
                return Some((dt, sim));
            }
            return None;
        }
    }
    let (sim, _wall) = state.run_clock.leg_duration_sim_and_wall(packet_id, now)?;
    if sim <= 0.05 {
        return None;
    }
    let dt = finalize_subsection_split_dt(state, sim, cfg);
    Some((dt, sim))
}

fn leg_distance_since_anchor(state: &LiveTimingState, distance_traveled_m: f64) -> f64 {
    state
        .run_clock
        .leg_distance_m(distance_traveled_m)
        .unwrap_or(0.0)
}

fn reset_subsection_leg_timing_accumulators(state: &mut LiveTimingState) {
    state.subsection_leg_excess_wall_sec = 0.0;
    state.subsection_timing_position_reset = false;
}

/// True once subsection / cumulative / stage / start-staging timing has begun.
fn live_timing_timer_running(state: &LiveTimingState) -> bool {
    if state.start_armed || state.run_clock.leg_anchor().is_some() {
        return true;
    }
    if state
        .cumulative
        .as_ref()
        .is_some_and(|c| c.last_gate_ix.is_some())
    {
        return true;
    }
    state
        .stage_sector_sessions
        .iter()
        .any(|s| s.run.armed && !s.run.completed)
}

fn near_track_start_point(
    idx: &HashMap<String, Vec<Point2>>,
    track_name: &str,
    p: Point2,
    radius_m: f64,
) -> bool {
    idx.get(track_name)
        .is_some_and(|pts| pts.iter().any(|sp| dist(*sp, p) <= radius_m))
}

/// Full timing reset at the grid (in-game stage restart). Does not arm — wait for Start cross / staging.
fn reset_live_timing_at_grid(
    state: &mut LiveTimingState,
    physics_hz: f64,
    car_model: &str,
    cum_def: Option<&CumulativeTrackSectors>,
    bus: &EventSender,
    store_path: &Path,
    reference_mode: acr_timing_store::ReferenceTimeMode,
) {
    state.run_clock = acr_timing::run_timing_clock::RunTimingClock::new(physics_hz);
    state.start_armed = false;
    state.start_stage_pos = None;
    state.start_stage_since = None;
    state.start_stage_last_report_sec = -1;
    state.last_sector_idx = None;
    state.overall_finish_recorded = false;
    reset_subsection_run(state);
    reset_subsection_leg_timing_accumulators(state);
    state.tracker = SectorPassTracker::new(state.ring_ids.len().max(1));

    if let Some(cum) = cum_def {
        state.cumulative = Some(acr_timing::cumulative_sector_timing::CumulativeLegState::new(
            cum.clone(),
        ));
        if let Some(m) = state.modular.as_mut() {
            m.presenter = PresenterState::default();
            m.coordinator.reset_run();
            m.coordinator.set_car(car_model);
        } else {
            ensure_modular_timing(
                state,
                bus,
                store_path,
                cum,
                &cum.reference_track,
                car_model,
                reference_mode,
            );
            if let Some(m) = state.modular.as_mut() {
                m.coordinator.reset_run();
            }
        }
    }

    for session in &mut state.stage_sector_sessions {
        session.run.reset_run();
    }

    eprintln!(
        "timing: reset at start grid (≥{START_GRID_TIMING_RESET_STILL_SEC:.0}s standstill, timer was running)"
    );
}

fn note_subsection_timing_position_reset(state: &mut LiveTimingState) {
    if !subsection_timing_active(state) || state.subsection_timing_position_reset {
        return;
    }
    state.subsection_timing_position_reset = true;
    eprintln!(
        "subsection: {}",
        acr_timing::stage_sector_timing::TIMING_POSITION_RESET_WARNING
    );
}

/// Apply optional stall-excess correction; reset per-leg accumulators after a committed split.
fn finalize_subsection_split_dt(
    state: &mut LiveTimingState,
    dt_raw: f64,
    cfg: &acr_timing::timing_frame_quality::TimingQualityConfig,
) -> f64 {
    let excess = state.subsection_leg_excess_wall_sec;
    let had_reset = state.subsection_timing_position_reset;
    let (dt, _) = if cfg.apply_leg_excess_correction {
        acr_timing::timing_frame_quality::TimingFrameMonitor::corrected_stage_leg_sec(
            dt_raw, excess,
        )
    } else {
        (dt_raw, 0.0)
    };
    if excess > 0.001 || had_reset {
        eprintln!(
            "subsection: raw={dt_raw:.3}s corrected={dt:.3}s stall_excess≈{excess:.3}s{}",
            if had_reset {
                format!(
                    " — {}",
                    acr_timing::stage_sector_timing::TIMING_POSITION_RESET_WARNING
                )
            } else {
                String::new()
            }
        );
    }
    state.subsection_leg_excess_wall_sec = 0.0;
    state.subsection_timing_position_reset = false;
    dt
}

fn format_subsection_split_line(
    from_id: i32,
    to_id: i32,
    dt: f64,
    dt_raw: f64,
    excess: f64,
    pending: bool,
    cfg: &acr_timing::timing_frame_quality::TimingQualityConfig,
) -> String {
    let pending_s = if pending { " (pending)" } else { "" };
    if cfg.apply_leg_excess_correction && (excess > 0.001 || (dt_raw - dt).abs() > 0.001) {
        format!(
            "sector [{from_id}]-[{to_id}]: {:.3}s (raw {:.3}s −{:.3}s ex){pending_s}",
            dt, dt_raw, excess.max(0.0),
        )
    } else {
        format!("sector [{from_id}]-[{to_id}]: {dt:.3}s{pending_s}")
    }
}

struct SubsectionSplitOutcome {
    line: String,
    leg_delta: f64,
    /// Δ vs PB for this leg only (before PB update); drives cumulative beep + RTSS flash.
    leg_pb_delta: Option<f64>,
    persisted: bool,
}

fn ensure_subsection_html_path(
    state: &mut LiveTimingState,
    cfg: &CliConfig,
    track_name: &str,
    car_model: &str,
) {
    if !cfg.subsection_html.enabled || state.subsection_html_path.is_some() {
        return;
    }
    let _ = std::fs::create_dir_all(&cfg.subsection_html_dir);
    let track_slug = acr_timing::subsection_split_html::track_slug(track_name);
    let car_slug = acr_timing::stage_sector_timing::sanitize_car_slug(car_model);
    state.subsection_html_path = Some(acr_timing::subsection_split_html::new_html_path(
        &cfg.subsection_html_dir,
        &track_slug,
        &car_slug,
    ));
}

fn commit_subsection_split(
    state: &mut LiveTimingState,
    conn: &rusqlite::Connection,
    pb: &mut acr_timing::timing_pb::TimingPbStore,
    cfg: &CliConfig,
    track_name: &str,
    car_model: &str,
    direction: &str,
    from_sector: i32,
    to_sector: i32,
    duration_sec: f64,
    duration_raw: f64,
    distance_m: f64,
    stats: Option<acr_timing::sector_leg_stats::SectorLegStatsSnapshot>,
    locked_track: Option<&str>,
    blame_ctx: Option<&TimingBlameCtx<'_>>,
    cumulative_pace: bool,
) -> SubsectionSplitOutcome {
    ensure_subsection_html_path(state, cfg, track_name, car_model);

    let leg_pb_delta = pb.leg_delta_for_feedback(
        track_name,
        car_model,
        direction,
        from_sector,
        to_sector,
        duration_sec,
    );

    state.subsection_run_legs.push((from_sector, to_sector, duration_sec));
    state.subsection_cumulative_sec += duration_sec;

    let split = acr_timing::timing_db::SplitRecord {
        track_name,
        car_model,
        direction,
        from_sector,
        to_sector,
        duration_sec,
        distance_m,
        stats,
    };

    let legs_for_pb: Vec<(i32, i32)> = state
        .subsection_run_legs
        .iter()
        .map(|(a, b, _)| (*a, *b))
        .collect();
    let cum_pb = pb.cumulative_best_time(track_name, car_model, direction, &legs_for_pb);
    let cum_delta = cum_pb.map(|pb| state.subsection_cumulative_sec - pb);

    let (line, leg_delta, persisted) = if let Some(locked) = locked_track {
        if locked == track_name {
            let (l, d) = persist_split_and_line(conn, pb, &split, blame_ctx);
            (l, d, true)
        } else {
            let _ = acr_timing::timing_db::insert_pending_split(conn, &split);
            (
                format_subsection_split_line(
                    from_sector,
                    to_sector,
                    duration_sec,
                    duration_raw,
                    duration_raw - duration_sec,
                    true,
                    &cfg.timing_quality,
                ),
                0.0,
                false,
            )
        }
    } else {
        let _ = acr_timing::timing_db::insert_pending_split(conn, &split);
        (
            format_subsection_split_line(
                from_sector,
                to_sector,
                duration_sec,
                duration_raw,
                duration_raw - duration_sec,
                true,
                &cfg.timing_quality,
            ),
            0.0,
            false,
        )
    };

    if cfg.subsection_html.enabled {
        if let Some(path) = state.subsection_html_path.as_ref() {
            let leg_d = if cumulative_pace {
                None
            } else if persisted {
                Some(leg_delta)
            } else {
                None
            };
            if let Err(e) = acr_timing::subsection_split_html::append_split_row(
                path,
                track_name,
                car_model,
                state.subsection_html_run_index,
                from_sector,
                to_sector,
                duration_sec,
                leg_d,
                state.subsection_cumulative_sec,
                cum_delta,
                cumulative_pace,
                !persisted,
            ) {
                eprintln!("subsection HTML: {e}");
            }
        }
    }

    SubsectionSplitOutcome {
        line,
        leg_delta,
        leg_pb_delta,
        persisted,
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
    rtss: bool,
    rtss_owner: String,
    rtss_slot: u32,
    rtss_clear_all: bool,
    rtss_osd_placement: acr_timing::rtss_osd::RtssOsdPlacement,
    sectors_shp: Option<PathBuf>,
    sectors_coord_space: SectorsCoordSpace,
    sector_track_field: String,
    sector_id_field: String,
    timing_db_path: PathBuf,
    timing_pb_path: PathBuf,
    timing_reference_store_path: PathBuf,
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
    beep_on_cumulative_split: bool,
    cumulative_beep: SplitBeepConfig,
    cumulative_timing: acr_timing::cumulative_timing_config::CumulativeTimingConfig,
    subsection_html: acr_timing::timing_config_file::SubsectionHtmlConfig,
    subsection_html_dir: PathBuf,
    pacenotes: Option<PacenoteConfig>,
    /// Calibrated stage-sector timing (independent of pacenotes).
    stage_timing: acr_timing::stage_timing_config::StageTimingConfig,
    /// Pearson correlations → timing_factors (see `[correlation]` in acr_timing.toml).
    correlation: acr_timing::timing_correlation::CorrelationConfig,
    timing_blame: acr_timing::timing_blame::BlameConfig,
    timing_voice: acr_timing::timing_voice::TimingVoiceConfig,
    timing_quality: acr_timing::timing_frame_quality::TimingQualityConfig,
    /// Live: print `{:#?}` of the last received physics map at most once per second.
    debug_physics_1hz: bool,
    /// `[zeitnahme]` stderr: positions, spielzeit, subsector vs sector sums.
    timing_debug: bool,
    delta_display: acr_timing::DeltaDisplayConfig,
    game_clock: acr_timing::game_clock_sync::GameClockSyncConfig,
    game_clock_sector: Option<acr_timing::game_clock_sector_override::GameClockSectorAdopterConfig>,
    reference_times: acr_timing::ReferenceTimesConfig,
    osd_templates: acr_timing::OsdTemplateConfig,
}

struct TimingBlameCtx<'a> {
    voice: Option<&'a acr_timing::timing_voice::TimingVoicePlayer>,
    cfg: &'a acr_timing::timing_blame::BlameConfig,
}


#[allow(clippy::too_many_arguments)]
fn apply_game_clock_finish_overrides(
    state: &mut LiveTimingState,
    si: usize,
    adopter: &mut acr_timing::game_clock_sector_override::GameClockSectorAdopter,
    game_clock: &mut acr_timing::game_clock_sync::GameClockCorrector,
    cfg: &CliConfig,
    timing_conn: &rusqlite::Connection,
    timing_pb: &mut acr_timing::timing_pb::TimingPbStore,
    car_model: &str,
    blame_ctx: &TimingBlameCtx<'_>,
    frame_monitor: &mut acr_timing::timing_frame_quality::TimingFrameMonitor,
    physics: &PhysicsMap,
    graphics_distance_traveled: f32,
) -> bool {
    if cfg.game_clock_sector.is_none() {
        return true;
    }
    adopter.poll_force();
    if game_clock.enabled() {
        game_clock.poll_now(Some(graphics_distance_traveled as f64));
    }
    let Some(sample) = adopter
        .cached_sample_owned()
        .or_else(|| game_clock.last_game_sample().cloned())
    else {
        return false;
    };
    let leg_count = state.stage_sector_sessions[si].markers.sector_leg_count;
    let orders =
        acr_timing::stage_sector_timing::stage_leg_pb_orders(&state.stage_sector_sessions[si].markers.markers);
    let slug = state.stage_sector_sessions[si].markers.stage_slug.clone();
    let label = state.stage_sector_sessions[si].markers.rtss_label().to_string();
    let overrides = {
        let session = &mut state.stage_sector_sessions[si];
        acr_timing::game_clock_sector_override::apply_finish_sector_overrides(&sample, session, true)
    };
    for o in &overrides {
        let (from_order, to_order) = orders.get(o.leg_ix).copied().unwrap_or((0, 0));
        eprintln!(
            "[{label}] Sektor-Übernahme Finish S{}: {:.3}s → Sektor-{}-Zeit (external timing provider) {:.3}s",
            o.leg_ix + 1,
            o.prev_sec,
            o.leg_ix + 1,
            o.provider_sec,
        );
        sync_modular_stage_cum_delta_from_brackets(
            state,
            timing_pb,
            car_model,
            cfg.delta_display.delta_scope,
            false,
        );
        frame_monitor.reset_leg_accumulator();
        state.stage_sector_sessions[si].run.leg_excess_wall_sec = 0.0;
        let leg_stats = take_leg_stats(state, physics.speed_kmh);
        let frozen = state.stage_sector_sessions[si].run.reference_secs_for_display();
        if let Ok((delta, pb)) = acr_timing::stage_sector_timing::archive_stage_leg(
            timing_conn,
            timing_pb,
            frozen,
            o.leg_ix,
            &slug,
            car_model,
            from_order,
            to_order,
            o.provider_sec,
            leg_stats,
        ) {
            if let Some(pb_sec) = pb {
                let split_rec = acr_timing::timing_db::SplitRecord {
                    track_name: &slug,
                    car_model,
                    direction: acr_timing::stage_sector_timing::STAGE_TIMING_DIRECTION,
                    from_sector: from_order,
                    to_sector: to_order,
                    duration_sec: o.provider_sec,
                    distance_m: 0.0,
                    stats: leg_stats,
                };
                maybe_timing_blame(timing_conn, &split_rec, pb_sec, delta, Some(blame_ctx));
            }
            let _ = delta;
        }
    }
    if !overrides.is_empty() {
        eprintln!(
            "[{label}] Sektor-Übernahme Finish: {}/{} Sektoren korrigiert (jsonl sectors={})",
            overrides.len(),
            leg_count,
            sample.sectors.len()
        );
    }
    let all_present = (0..leg_count).all(|leg_ix| {
        acr_timing::game_clock_sector_override::sector_leg_split_sec_for_finish(
            &sample, leg_ix, leg_count,
        )
        .is_some()
    });
    if !all_present {
        adopter.enqueue_finish_retry(si, label, slug, leg_count);
    }
    all_present
}

#[allow(dead_code)]
fn prior_stage_splits(state: &LiveTimingState, session_si: usize, leg_ix: usize) -> Vec<f64> {
    state
        .stage_sector_sessions
        .get(session_si)
        .map(|sess| {
            sess.run
                .sector_secs
                .iter()
                .take(leg_ix)
                .filter_map(|t| *t)
                .collect()
        })
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn process_stage_sector_sessions_on_step(
    state: &mut LiveTimingState,
    lp: Point2,
    p: Point2,
    cfg: &CliConfig,
    timing_conn: &rusqlite::Connection,
    timing_pb: &mut acr_timing::timing_pb::TimingPbStore,
    timing_reference_store: Option<&ReferenceStore>,
    physics: &PhysicsMap,
    packet_id: i32,
    physics_hz: f64,
    graphics_distance_traveled: f32,
    graphics_current_time_ms: i32,
    car_model: &str,
    locked_track: Option<&str>,
    blame_ctx: &TimingBlameCtx<'_>,
    frame_monitor: &mut acr_timing::timing_frame_quality::TimingFrameMonitor,
    stage_sector_html_run_counter: &mut usize,
    speed_kmh_now: f64,
    game_clock_sector: &mut Option<acr_timing::game_clock_sector_override::GameClockSectorAdopter>,
    game_clock: &mut acr_timing::game_clock_sync::GameClockCorrector,
) {
    if state.stage_sector_sessions.is_empty() {
        return;
    }
    let now_inst = Instant::now();
    if game_clock.enabled() {
        game_clock.poll_now(Some(graphics_distance_traveled as f64));
    }
    let game_race_hud = game_clock.game_race_for_sector_display();
    let jsonl_fresh_for_adopt = game_clock.jsonl_fresh_for_display();
    if let Some(adopter) = game_clock_sector.as_mut() {
        if jsonl_fresh_for_adopt {
            let finish_overrides =
                acr_timing::game_clock_sector_override::drain_finish_overrides(
                    adopter,
                    now_inst,
                    &mut state.stage_sector_sessions,
                );
            for o in finish_overrides {
                let label = state
                    .stage_sector_sessions
                    .get(o.session_si)
                    .map(|s| s.markers.rtss_label())
                    .unwrap_or("stage");
                eprintln!(
                    "[{label}] Sektor-Übernahme Finish S{} (nachgereicht): {:.3}s → Sektor-{}-Zeit (external timing provider) {:.3}s",
                    o.leg_ix + 1,
                    o.prev_sec,
                    o.leg_ix + 1,
                    o.provider_sec,
                );
                sync_modular_stage_cum_delta_from_brackets(
                    state,
                    timing_pb,
                    car_model,
                    cfg.delta_display.delta_scope,
                    false,
                );
            }
            let commits = adopter.drain_commit_ready(now_inst, &state.stage_sector_sessions);
            for c in commits {
                let s = c.leg_ix + 1;
                if c.via == "gate" {
                    eprintln!(
                        "[{}] Sektor-Übernahme S{s}: FEHLGESCHLAGEN — Sektor-{s}-Zeit (external timing provider) nach {:.1}s nicht verfügbar; Gate-Zeit {:.3}s bleibt",
                        c.label,
                        adopter.cfg.adopt_window_sec,
                        c.duration_sec,
                    );
                } else if c.via == "game_clock" && (c.duration_sec - c.gate_dt).abs() > 0.001 {
                    eprintln!(
                        "[{}] Sektor-Übernahme S{s}: Gate {:.3}s → Sektor-{s}-Zeit (external timing provider) {:.3}s (verzögert)",
                        c.label,
                        c.gate_dt,
                        c.duration_sec,
                    );
                }
                if c.leg_ix < state.stage_sector_sessions[c.session_si].run.sector_secs.len() {
                    state.stage_sector_sessions[c.session_si].run.sector_secs[c.leg_ix] =
                        Some(c.duration_sec);
                }
                sync_modular_stage_cum_delta_from_brackets(
                    state,
                    timing_pb,
                    car_model,
                    cfg.delta_display.delta_scope,
                    false,
                );
                frame_monitor.reset_leg_accumulator();
                state.stage_sector_sessions[c.session_si].run.leg_excess_wall_sec = 0.0;
                let leg_stats = take_leg_stats(state, physics.speed_kmh);
                let frozen = state.stage_sector_sessions[c.session_si]
                    .run
                    .reference_secs_for_display();
                let _ = acr_timing::stage_sector_timing::archive_stage_leg(
                    &timing_conn,
                    timing_pb,
                    frozen,
                    c.leg_ix,
                    &c.slug,
                    car_model,
                    c.from_order,
                    c.to_order,
                    c.duration_sec,
                    leg_stats,
                );
            }
        }
    }
    let stage_radius = cfg.stage_timing.stage_sector_radius_m();
    let session_count = state.stage_sector_sessions.len();
    for si in 0..session_count {
        let outcome = {
            let session = &mut state.stage_sector_sessions[si];
            acr_timing::stage_sector_timing::observe_stage_crossing(
                session,
                (lp.x, lp.z),
                (p.x, p.z),
                stage_radius,
                packet_id,
                physics_hz,
                now_inst,
                game_race_hud,
            )
        };
        let Some(outcome) = outcome else {
            continue;
        };
        let leg_recorded = outcome.leg_duration_sec;
        let session_excess = state.stage_sector_sessions[si].run.leg_excess_wall_sec;
        let (leg_dt, leg_excess) = leg_recorded
            .map(|raw| {
                if cfg.timing_quality.apply_leg_excess_correction {
                    acr_timing::timing_frame_quality::TimingFrameMonitor::corrected_stage_leg_sec(
                        raw,
                        session_excess,
                    )
                } else {
                    (raw, 0.0)
                }
            })
            .unwrap_or((0.0, 0.0));
        let slug = state.stage_sector_sessions[si].markers.stage_slug.clone();
        let label = state.stage_sector_sessions[si].markers.rtss_label().to_string();
        if let Some(crossed) = state.stage_sector_sessions[si]
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
            if cfg.timing_debug
                && acr_timing::stage_sector_timing::stage_marker_is_main_sector(crossed)
            {
                acr_timing::timing_debug::log_stage_sektor_zeit(
                    crossed,
                    via,
                    leg_dt,
                    leg_recorded,
                    stage_sektoren_summe_sec(state),
                    state.subsection_cumulative_sec,
                    state.run_clock.run_sim_sec(packet_id),
                    acr_timing::timing_debug::spielzeit_sec(graphics_current_time_ms),
                    physics,
                    p.x,
                    p.z,
                    speed_kmh_now as f32,
                    graphics_distance_traveled,
                );
            } else if cfg.timing_debug {
                acr_timing::physics_wheel::log_sector_crossing(
                    &crossed.label,
                    crossed.role.as_str(),
                    via,
                    physics,
                    p.x,
                    p.z,
                    speed_kmh_now,
                    leg_recorded.map(|_| leg_dt),
                    graphics_distance_traveled,
                    crossed.x,
                    crossed.z,
                );
            }
        }
        let run_completed = outcome.run_completed;
        if leg_recorded.is_some() {
            let dt_raw = leg_recorded.unwrap();
            if let Some(leg_ix) = outcome.leg_index {
                let prior = acr_timing::game_clock_sector_override::prior_splits_from_sessions(
                    &state.stage_sector_sessions,
                    si,
                    leg_ix,
                );
                let session = &mut state.stage_sector_sessions[si];
                let mut adopt_dt = leg_dt;
                let s_num = leg_ix + 1;
                if cfg.game_clock_sector.is_some() {
                    if !jsonl_fresh_for_adopt {
                        eprintln!(
                            "[{label}] Sektor-Übernahme S{s_num}: keine frische acr_game_clock.jsonl — Sektor-{s_num}-Zeit (external timing provider) nicht verfügbar; vorläufig Gate-Zeit {leg_dt:.3}s"
                        );
                    } else if let Some(adopter) = game_clock_sector.as_mut() {
                        adopter.poll_force();
                        if let Some(split) =
                            acr_timing::game_clock_sector_override::try_external_timing_leg_split_sec(
                                game_clock,
                                Some(adopter),
                                leg_ix,
                                &prior,
                            )
                        {
                            if (split - leg_dt).abs() > 0.001 {
                                eprintln!(
                                    "[{label}] Sektor-Übernahme S{s_num}: Gate {leg_dt:.3}s → Sektor-{s_num}-Zeit (external timing provider) {split:.3}s"
                                );
                            }
                            adopt_dt = split;
                        } else if adopter.is_live() && !run_completed {
                            let poll_iv =
                                Duration::from_secs_f64(adopter.cfg.poll_interval_sec);
                            let window =
                                Duration::from_secs_f64(adopter.cfg.adopt_window_sec);
                            let reason = game_clock
                                .last_game_sample()
                                .map(|sample| {
                                    let dbg = sample
                                        .sectors_debug
                                        .as_ref()
                                        .map(|d| {
                                            format!(
                                                "sectors_debug array_num={:?} parsed={:?} first_err={:?}",
                                                d.array_num, d.parsed, d.first_err
                                            )
                                        })
                                        .unwrap_or_default();
                                    if sample.sectors.is_empty() {
                                        if dbg.is_empty() {
                                            "sectors[] leer (evtl. nur light-Sample ohne Merge)".to_string()
                                        } else {
                                            format!("sectors[] leer; {dbg}")
                                        }
                                    } else if !acr_timing::game_clock_sector_override::leg_ready_in_sample(
                                        sample, leg_ix,
                                    ) {
                                        format!(
                                            "nur {} sector-Einträge, next_sector_index={:?}{}",
                                            sample.sectors.len(),
                                            sample.next_sector_index,
                                            if dbg.is_empty() {
                                                String::new()
                                            } else {
                                                format!("; {dbg}")
                                            }
                                        )
                                    } else {
                                        format!("split_s/time_s fehlt oder ungültig{dbg}")
                                    }
                                })
                                .unwrap_or_else(|| "kein jsonl-Sample".to_string());
                            eprintln!(
                                "[{label}] Sektor-Übernahme S{s_num}: Sektor-{s_num}-Zeit (external timing provider) noch nicht da ({reason}) — warte bis {:.1}s, vorläufig Gate {leg_dt:.3}s",
                                adopter.cfg.adopt_window_sec
                            );
                            adopter.enqueue(
                                acr_timing::game_clock_sector_override::PendingSectorAdopt {
                                    session_si: si,
                                    leg_ix,
                                    gate_dt: leg_dt,
                                    dt_raw,
                                    leg_excess: 0.0,
                                    from_order: outcome.from_order,
                                    to_order: outcome.to_order,
                                    slug: slug.clone(),
                                    label: label.clone(),
                                    pass_method: outcome.pass_method,
                                    created: now_inst,
                                    next_poll: now_inst + poll_iv,
                                    deadline: now_inst + window,
                                },
                            );
                        } else if run_completed {
                            let leg_count = session.markers.sector_leg_count;
                            let finish_split = adopter
                                .cached_sample()
                                .or_else(|| game_clock.last_game_sample())
                                .and_then(|sample| {
                                    acr_timing::game_clock_sector_override::sector_leg_split_sec_for_finish(
                                        sample,
                                        leg_ix,
                                        leg_count,
                                    )
                                });
                            if let Some(split) = finish_split {
                                if (split - leg_dt).abs() > 0.001 {
                                    eprintln!(
                                        "[{label}] Sektor-Übernahme S{s_num}: Gate {leg_dt:.3}s → Sektor-{s_num}-Zeit (external timing provider) {split:.3}s"
                                    );
                                }
                                adopt_dt = split;
                            } else {
                                eprintln!(
                                    "[{label}] Sektor-Übernahme S{s_num}: Sektor-{s_num}-Zeit (external timing provider) noch nicht in jsonl — vorläufig Gate-Zeit {leg_dt:.3}s (Finish-Nachzug)"
                                );
                                adopter.enqueue_finish_retry(
                                    si,
                                    label.clone(),
                                    slug.clone(),
                                    leg_count,
                                );
                            }
                        } else {
                            eprintln!(
                                "[{label}] Sektor-Übernahme S{s_num}: jsonl-Adopter nicht live — Sektor-{s_num}-Zeit (external timing provider) nicht verfügbar; vorläufig Gate-Zeit {leg_dt:.3}s"
                            );
                        }
                    }
                }
                if leg_ix < session.run.sector_secs.len() {
                    session.run.sector_secs[leg_ix] = Some(adopt_dt);
                }
                sync_modular_stage_cum_delta_from_brackets(
                    state,
                    timing_pb,
                    car_model,
                    cfg.delta_display.delta_scope,
                    true,
                );
            }
            if let Some(summary) = frame_monitor.leg_close_summary(
                &format!("[{label}]"),
                dt_raw,
                if cfg.timing_quality.apply_leg_excess_correction {
                    Some(leg_dt)
                } else {
                    None
                },
                session_excess,
            ) {
                eprintln!("{summary}");
            }
            frame_monitor.reset_leg_accumulator();
            state.stage_sector_sessions[si].run.leg_excess_wall_sec = 0.0;
            let exit_speed = physics.speed_kmh;
            let leg_stats = take_leg_stats(state, exit_speed);
            let leg_ix = outcome.leg_index.unwrap_or(0);
            let duration_sec = state.stage_sector_sessions[si]
                .run
                .sector_secs
                .get(leg_ix)
                .and_then(|t| *t)
                .unwrap_or(leg_dt);
            let frozen = state.stage_sector_sessions[si].run.reference_secs_for_display();
            match acr_timing::stage_sector_timing::archive_stage_leg(
                &timing_conn,
                timing_pb,
                frozen,
                leg_ix,
                &slug,
                car_model,
                outcome.from_order,
                outcome.to_order,
                duration_sec,
                leg_stats,
            ) {
                Ok((delta, pb)) => {
                    let detail = if leg_excess > 0.001 {
                        outcome.leg_index.map(|leg_ix| {
                            let via = outcome
                                .pass_method
                                .map(|m| match m {
                                    acr_timing::timing_sectors::GatePassMethod::GateLine => {
                                        "gate_line"
                                    }
                                    acr_timing::timing_sectors::GatePassMethod::RadiusDisc => {
                                        "radius_disc"
                                    }
                                })
                                .unwrap_or("?");
                            format!(
                                "stage S{}: {} ({via}) [raw {} −{:.3}s ex]",
                                leg_ix + 1,
                                acr_timing::stage_sector_timing::format_duration(leg_dt),
                                acr_timing::stage_sector_timing::format_duration(dt_raw),
                                leg_excess,
                            )
                        })
                    } else {
                        outcome.overlay_detail.clone()
                    };
                    if let Some(detail) = detail {
                        let line = format!("[{label}] {detail}");
                        eprintln!("{line}");
                    }
                    let main_sector_leg = state.stage_sector_sessions[si]
                        .markers
                        .markers
                        .iter()
                        .find(|m| m.order == outcome.to_order)
                        .is_some_and(acr_timing::stage_sector_timing::stage_marker_is_main_sector);
                    if cfg.beep_on_split && !main_sector_leg {
                        acr_timing::split_beep::play_split_feedback(delta, &cfg.split_beep);
                    }
                    if let Some(pb_sec) = pb {
                        let split = acr_timing::timing_db::SplitRecord {
                            track_name: &slug,
                            car_model,
                            direction: acr_timing::stage_sector_timing::STAGE_TIMING_DIRECTION,
                            from_sector: outcome.from_order,
                            to_sector: outcome.to_order,
                            duration_sec: leg_dt,
                            distance_m: 0.0,
                            stats: leg_stats,
                        };
                        maybe_timing_blame(
                            timing_conn,
                            &split,
                            pb_sec,
                            delta,
                            Some(blame_ctx),
                        );
                    }
                }
                Err(e) => eprintln!("stage sector DB: {}", e),
            }
        } else if let Some(detail) = outcome.overlay_detail.clone() {
            let line = format!("[{label}] {detail}");
            eprintln!("{line}");
            if detail.contains("armed") {
                let ref_track = locked_track.unwrap_or(slug.as_str());
                freeze_stage_run_references(
                    &mut state.stage_sector_sessions[si],
                    cfg,
                    timing_pb,
                    timing_reference_store,
                    ref_track,
                    car_model,
                );
                state.stage_sector_sessions[si].run.leg_excess_wall_sec = 0.0;
                frame_monitor.reset_leg_accumulator();
            }
        }
        if run_completed {
            if let Some(adopter) = game_clock_sector.as_mut() {
                if jsonl_fresh_for_adopt {
                    let _ = apply_game_clock_finish_overrides(
                        state,
                        si,
                        adopter,
                        game_clock,
                        cfg,
                        timing_conn,
                        timing_pb,
                        car_model,
                        blame_ctx,
                        frame_monitor,
                        physics,
                        graphics_distance_traveled,
                    );
                }
            }
            let session = &mut state.stage_sector_sessions[si];
            flush_one_stage_sector_session(
                session,
                cfg,
                timing_pb,
                car_model,
                stage_sector_html_run_counter,
            );
        }
    }
}

fn maybe_timing_blame(
    conn: &rusqlite::Connection,
    split: &acr_timing::timing_db::SplitRecord<'_>,
    pb_duration_sec: f64,
    delta_sec: f64,
    ctx: Option<&TimingBlameCtx<'_>>,
) {
    let Some(ctx) = ctx else { return };
    match acr_timing::timing_blame::analyze_slower_split(
        conn,
        split,
        pb_duration_sec,
        delta_sec,
        ctx.cfg,
    ) {
        Ok(Some(result)) => {
            eprintln!("timing blame: {}", result.summary_line());
            if let Some(v) = ctx.voice {
                v.enqueue(result.voice_tokens(ctx.cfg.max_factors));
            }
        }
        Ok(None) => {}
        Err(e) => eprintln!("timing blame: {e}"),
    }
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
    let mut rtss = tm.rtss.unwrap_or(false);
    let mut rtss_owner = tm
        .rtss_owner
        .clone()
        .unwrap_or_else(|| "acr_track_match".to_string());
    let mut rtss_slot = tm.rtss_slot.unwrap_or(0u32);
    let mut rtss_clear_all = tm.rtss_clear_all.unwrap_or(false);
    let mut rtss_osd_placement = acr_timing::rtss_osd::RtssOsdPlacement::from_config(
        tm.rtss_osd_anchor.as_deref(),
        tm.rtss_osd_offset_x,
        tm.rtss_osd_offset_y,
        tm.rtss_osd_x,
        tm.rtss_osd_y,
    );
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
    let timing_pb_path: Option<PathBuf> = timing.timing_pb.as_ref().map(PathBuf::from);
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
    let beep_on_cumulative_split = timing
        .beep_on_cumulative_split
        .or(timing.beep_on_silent_split)
        .unwrap_or(true);
    let cumulative_beep = timing
        .cumulative_beep
        .clone()
        .or(timing.silent_beep.clone())
        .unwrap_or_else(|| SplitBeepConfig {
            faster_freq_hz: 660.0,
            faster_duration_ms: 50,
            slower_freq_hz: 330.0,
            slower_duration_ms: 160,
            gap_ms: 220,
            volume: split_beep.volume,
            ..Default::default()
        });
    let cumulative_timing = timing.cumulative_timing.clone();
    let subsection_html = timing.subsection_html.clone();
    #[cfg(feature = "acr_timing_bin")]
    let pacenotes = None;
    #[cfg(not(feature = "acr_timing_bin"))]
    let pacenotes = loaded.pacenotes.clone();
    let mut debug_physics_1hz = tm.debug_physics_1hz.unwrap_or(false);
    let stage_timing = timing.stage_timing.clone();
    let subsection_html_dir = PathBuf::from(
        subsection_html
            .dir
            .as_deref()
            .or(stage_timing.timing_sectors_html_dir.as_deref())
            .unwrap_or("timing/runs"),
    );
    let correlation = timing.correlation.to_runtime();
    let timing_blame = timing.timing_blame.to_runtime(&correlation);
    let timing_voice = timing.timing_voice.clone();
    let timing_quality = timing.timing_quality.to_runtime();
    let delta_display = timing.delta_display.to_runtime();
    let mut timing_debug = timing.timing_debug;

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
            "--rtss-osd-anchor" => {
                rtss_osd_placement.anchor = args
                    .get(i + 1)
                    .ok_or("--rtss-osd-anchor needs default|middle_monitor|pixel")?
                    .to_string();
                i += 1;
            }
            "--rtss-osd-offset-x" => {
                rtss_osd_placement.offset_x = args
                    .get(i + 1)
                    .ok_or("--rtss-osd-offset-x needs integer")?
                    .parse()?;
                i += 1;
            }
            "--rtss-osd-offset-y" => {
                rtss_osd_placement.offset_y = args
                    .get(i + 1)
                    .ok_or("--rtss-osd-offset-y needs integer")?
                    .parse()?;
                i += 1;
            }
            "--rtss-osd-x" => {
                rtss_osd_placement.pixel_x = Some(
                    args.get(i + 1)
                        .ok_or("--rtss-osd-x needs integer")?
                        .parse()?,
                );
                i += 1;
            }
            "--rtss-osd-y" => {
                rtss_osd_placement.pixel_y = Some(
                    args.get(i + 1)
                        .ok_or("--rtss-osd-y needs integer")?
                        .parse()?,
                );
                i += 1;
            }
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
            "--timing-debug" => timing_debug = true,
            "--no-timing-debug" => timing_debug = false,
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

    let timing_db_path = timing_db_path.unwrap_or_else(|| {
        let cfg = config::load_config();
        config::resolve_notes_dir(&cfg.recorder).join("timing.db")
    });
    let timing_pb_path = timing_pb_path.unwrap_or_else(|| {
        timing_db_path
            .parent()
            .map(|d| d.join("timing_pb.toml"))
            .unwrap_or_else(|| PathBuf::from("timing/timing_pb.toml"))
    });
    let timing_reference_store_path = timing_pb_path
        .parent()
        .map(|d| d.join("reference_runs.sqlite"))
        .unwrap_or_else(|| PathBuf::from("timing/reference_runs.sqlite"));

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
        rtss,
        rtss_owner,
        rtss_slot,
        rtss_clear_all,
        rtss_osd_placement,
        sectors_shp,
        sectors_coord_space,
        sector_track_field,
        sector_id_field,
        timing_db_path,
        timing_pb_path,
        timing_reference_store_path,
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
        beep_on_cumulative_split,
        cumulative_beep,
        cumulative_timing,
        subsection_html,
        subsection_html_dir,
        pacenotes,
        stage_timing,
        correlation,
        timing_blame,
        timing_voice,
        timing_quality,
        debug_physics_1hz,
        timing_debug,
        game_clock: timing.game_clock.to_runtime(),
        game_clock_sector: timing.game_clock.sector_adopter_config(),
        reference_times: timing.reference_times.to_runtime(),
        osd_templates: timing.osd_display.to_runtime(),
        delta_display,
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
    eprintln!("       --rtss                 Push message to RTSS OSD (Windows)");
    eprintln!("       --rtss-owner NAME      RTSS OSD owner id (default: acr_track_match)");
    eprintln!("       --rtss-slot N          Force RTSS slot N (0 = auto, default: 0)");
    eprintln!("       --rtss-clear-all       Clear all RTSS slots once at startup (careful: clears other OSD sources)");
    eprintln!("       --rtss-osd-anchor A    default | middle_monitor | pixel (TOML: rtss_osd_anchor)");
    eprintln!("       --rtss-osd-offset-x N  Horizontal nudge after anchor (virtual px)");
    eprintln!("       --rtss-osd-offset-y N  Vertical nudge (negative = up)");
    eprintln!("       --rtss-osd-x N         Absolute X (with --rtss-osd-y)");
    eprintln!("       --rtss-osd-y N         Absolute Y (virtual desktop, top-left origin)");
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
    eprintln!("       --beep-on-split        Play split feedback (sine/WAV — [beep] in acr_timing.toml)");
    eprintln!("       --debug-physics-1hz    Live: stderr dump of last PhysicsMap (~1/s, Rust pretty-Debug)");
    eprintln!("       --timing-debug / --no-timing-debug  Override [timing_debug] in acr_timing.toml");
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
    if let Some(ref cb) = loaded.timing.cumulative_beep {
        acr_timing::split_beep::log_wav_paths("cumulative_beep", cb);
    }
    if let Some(ref b) = loaded.timing.beep {
        acr_timing::split_beep::log_wav_paths("beep", b);
    }
    if loaded.track_match.rtss.unwrap_or(false) {
        let p = acr_timing::rtss_osd::RtssOsdPlacement::from_config(
            loaded.track_match.rtss_osd_anchor.as_deref(),
            loaded.track_match.rtss_osd_offset_x,
            loaded.track_match.rtss_osd_offset_y,
            loaded.track_match.rtss_osd_x,
            loaded.track_match.rtss_osd_y,
        );
        eprintln!("rtss_osd: placement {}", p.describe());
    }
    let osd_rt = loaded.timing.osd_display.to_runtime();
    eprintln!(
        "osd_display: preset={:?} live_delta_font_scale={}",
        osd_rt.preset, osd_rt.live_delta_font_scale
    );
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
        Some(LiveTimingState::new_preserving_cumulative(
            s.ring_ids.clone(),
            timing_state.take(),
            track_name,
        ))
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
            None,
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
    acr_timing::timing_db::set_correlation_config(cfg.correlation.clone());
    let timing_conn = acr_timing::timing_db::open_or_create(&cfg.timing_db_path)?;
    let mut timing_pb = acr_timing::timing_pb::TimingPbStore::load(&cfg.timing_pb_path)?;
    if timing_pb.is_empty() {
        match timing_pb.import_from_db(&timing_conn) {
            Ok(n) if n > 0 => eprintln!(
                "timing_pb: seeded {} leg PB(s) from {} → {}",
                n,
                cfg.timing_db_path.display(),
                timing_pb.path().display()
            ),
            Ok(_) => {}
            Err(e) => eprintln!("timing_pb import from db: {e}"),
        }
    } else {
        eprintln!(
            "timing_pb: {} leg PB(s) from {}",
            timing_pb.len(),
            timing_pb.path().display()
        );
    }
    let timing_reference_store = match ReferenceStore::open(&cfg.timing_reference_store_path) {
        Ok(store) => {
            eprintln!(
                "timing_reference_store: open {} ([reference_times] mode={})",
                cfg.timing_reference_store_path.display(),
                cfg.reference_times.mode.as_str()
            );
            Some(store)
        }
        Err(e) => {
            eprintln!(
                "timing_reference_store: could not open {} ({e})",
                cfg.timing_reference_store_path.display()
            );
            None
        }
    };
    let timing_voice = cfg
        .timing_voice
        .voice_dir
        .as_ref()
        .filter(|_| cfg.timing_voice.enabled)
        .map(|dir| {
            acr_timing::timing_voice::TimingVoicePlayer::spawn(
                dir.clone(),
                cfg.timing_voice.volume,
            )
        });
    let blame_ctx = TimingBlameCtx {
        voice: timing_voice.as_ref(),
        cfg: &cfg.timing_blame,
    };
    if cfg.timing_blame.enabled || cfg.timing_voice.copilot_crash_voice {
        if let Some(dir) = cfg.timing_voice.voice_dir.as_ref() {
            eprintln!(
                "timing voice: {} (enabled={}, copilot_crash={})",
                dir.display(),
                cfg.timing_voice.enabled,
                cfg.timing_voice.copilot_crash_voice
            );
        } else if cfg.timing_voice.enabled {
            eprintln!("timing voice enabled but voice_dir missing in [timing_voice]");
        }
    }
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
    let cumulative_tracks = if !cfg.cumulative_timing.ref_track_sectors.is_empty() {
        acr_timing::cumulative_sector_timing::load_all(&cfg.cumulative_timing)?
    } else {
        HashMap::new()
    };
    let timing_event_bus = acr_timing_protocol::EventSender::new();
    eprintln!(
        "timing_reference_store: {}",
        cfg.timing_reference_store_path.display()
    );
    let mut sector_sets = sector_sets;
    for key in cumulative_tracks.keys() {
        if sector_sets.remove(key).is_some() {
            eprintln!(
                "subsection SHP: disabled for '{key}' (cumulative GeoJSON active)"
            );
        }
    }
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
    let mut grid_timing_reset_still_since: Option<Instant> = None;
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
    let mut frame_monitor =
        acr_timing::timing_frame_quality::TimingFrameMonitor::new(cfg.timing_quality.clone());
    let mut game_clock = acr_timing::game_clock_sync::GameClockCorrector::new(cfg.game_clock.clone());
    let mut pause_osd = acr_timing::game_clock_sync::PauseOsdState::default();
    let mut last_physics_packet_id: Option<i32> = None;
    if game_clock.enabled() {
        eprintln!(
            "game_clock: 1-Hz-Zeitkorrektur (Replik-Spielzeit), jsonl={}",
            cfg.game_clock.jsonl_path.display()
        );
    }
    let mut game_clock_sector = cfg
        .game_clock_sector
        .clone()
        .map(acr_timing::game_clock_sector_override::GameClockSectorAdopter::new);
    if game_clock_sector.is_some() {
        eprintln!(
            "game_clock: Sektor-Übernahme aktiv (sector_splits=true) — Sektor-i-Zeit (external timing provider) an S1/S2/S3/Finish"
        );
    } else if game_clock.enabled() {
        eprintln!(
            "game_clock: Sektor-Übernahme AUS (sector_splits=false) — Live-Sektor nur via Replik"
        );
    }
    let mut copilot_crash_voice = acr_timing::timing_voice::CopilotCrashVoiceState::default();
    let _ = std::fs::create_dir_all(&cfg.stage_timing.html_dir());
    // After Ctrl+Enter on the first-anchor picker, suppress reopening the menu while the same
    // anchors stay within radius (otherwise the overlay immediately reappears and Enter retriggers).
    let mut pacenote_manual_anchor_slug: Option<String> = None;
    let mut last_rtss_msg = String::new();
    let mut last_rtss_push = Instant::now();
    #[cfg(windows)]
    {
        if cfg.rtss {
            // Always release our own owner on startup to avoid stale slot artifacts from prior runs.
            let _ = acr_timing::rtss_osd::release(&cfg.rtss_owner);
            // Demo binary uses a separate owner but often the same slot (0).
            let _ = acr_timing::rtss_osd::release("acr_timing_demo");
            if cfg.rtss_clear_all {
                match acr_timing::rtss_osd::clear_all() {
                    Ok(()) => eprintln!("RTSS cleanup: cleared all OSD slots."),
                    Err(e) => eprintln!("RTSS cleanup failed: {}", e),
                }
            }
        }
    }
    push_rtss_osd(cfg, "")?;
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
            let pkt = data.physics.packet_id;
            let dt_sim = match last_physics_packet_id {
                Some(prev) if pkt >= prev => (pkt - prev) as f64 / cfg.timing_quality.physics_hz,
                _ => 0.0,
            };
            last_physics_packet_id = Some(pkt);
            if dt_sim > 0.0 {
                game_clock.tick(
                    dt_sim,
                    Some(data.graphics.distance_traveled as f64),
                );
            }
            if cfg.timing_debug && game_clock.enabled() {
                let stage_anchor = timing_state.as_ref().and_then(|s| {
                    s.stage_sector_sessions
                        .iter()
                        .find(|sess| sess.run.armed && !sess.run.completed)
                        .and_then(|sess| sess.run.game_race_anchor_sec)
                });
                game_clock.maybe_log_sync_debug(stage_anchor);
            }
            let tick_stall_excess = frame_monitor.tick_timing_excess(&data.physics);
            if tick_stall_excess > 0.0 && !game_clock.timing_frozen() {
                if let Some(state) = timing_state.as_mut() {
                    for session in &mut state.stage_sector_sessions {
                        if session.run.armed && !session.run.completed {
                            session.run.leg_excess_wall_sec += tick_stall_excess;
                        }
                    }
                    if subsection_timing_active(state) {
                        state.subsection_leg_excess_wall_sec += tick_stall_excess;
                    }
                }
            }
            if frame_monitor.tick_position_reset_suspect(&data.physics) {
                if let Some(state) = timing_state.as_mut() {
                    note_stage_timing_position_reset(state);
                    note_subsection_timing_position_reset(state);
                }
            }
            let stage_copilot_active = timing_state.as_ref().is_some_and(|s| {
                s.stage_sector_sessions
                    .iter()
                    .any(|sess| sess.run.armed && !sess.run.completed)
            });
            let speed_kmh_now = data.physics.speed_kmh as f64;
            if stage_copilot_active {
                copilot_crash_voice.observe_high_g(&data.physics.g_force, &cfg.timing_voice);
                copilot_crash_voice.observe_speed_for_pending_copilot(
                    speed_kmh_now,
                    timing_voice.as_ref(),
                    &cfg.timing_voice,
                );
            } else {
                copilot_crash_voice.clear_copilot_pending();
            }
            if let Some(line) = frame_monitor.observe_physics(&data.physics) {
                eprintln!("{line}");
            }
            let car_model_now = data.statics.car_model.trim().to_string();
            if let Some(lock_car) = locked_car_model.clone() {
                if !car_model_now.is_empty() && car_model_now != lock_car {
                    eprintln!(
                        "unlocking track lock due to car change: '{}' -> '{}'",
                        lock_car, car_model_now
                    );
                    if let Some(state) = timing_state.as_mut() {
                        flush_all_stage_sector_sessions(
                            state,
                            &cfg,
                            &mut timing_pb,
                            &lock_car,
                            &mut stage_sector_html_run_counter,
                        );
                    }
                    locked_track = None;
                    locked_car_model = None;
                    stable_selected = None;
                    active_track_name = None;
                    timing_state = None;
                    game_clock.reset();
                    pause_osd.reset();
                    active_timing_stage_slug = None;
                    latest_timing_line = None;
                    sector_status_line = Some((
                        format!("reset after car change ('{lock_car}' -> '{car_model_now}')"),
                        Instant::now(),
                    ));
                    detected_track_line = None;
                    history.clear();
                    last_pt = None;
                    total_drive_m = 0.0;
                    pacenote_ambiguous_pick = None;
                    start_track_ambiguous_pick = None;
                    grid_standstill_since = None;
                    grid_timing_reset_still_since = None;
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
                    let _ = push_rtss_osd(cfg, "");
                    last_rtss_msg.clear();
                    last_rtss_push = Instant::now();
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
                .and_then(|ts| ts.stage_sector_sessions.first())
                .map(|sess| {
                    let now = Instant::now();
                    let next = sess
                        .markers
                        .markers
                        .get(sess.run.next_marker_idx)
                        .map(|m| m.label.as_str());
                    (
                        sess.run.armed,
                        sess.run.live_leg_elapsed_sec(now, game_clock.game_race_for_sector_display()),
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
                    graphics_clock: f64::NAN,
                    distance_traveled_m: data.graphics.distance_traveled as f64,
                    stage_armed,
                    stage_leg_elapsed_sec: stage_leg_elapsed,
                    stage_next_label,
                },
            );
            if let Some(track_name) = locked_track.as_deref() {
                if let Some(state) = timing_state.as_mut() {
                    if live_timing_timer_running(state)
                        && speed_kmh_now <= cfg.grid_standstill_max_speed_kmh
                        && near_track_start_point(
                            &start_index,
                            track_name,
                            p,
                            cfg.grid_start_trigger_radius_m,
                        )
                    {
                        if grid_timing_reset_still_since.is_none() {
                            grid_timing_reset_still_since = Some(Instant::now());
                        } else if grid_timing_reset_still_since
                            .unwrap()
                            .elapsed()
                            .as_secs_f64()
                            >= START_GRID_TIMING_RESET_STILL_SEC
                        {
                            let car_model = if car_model_now.is_empty() {
                                "unknown_car"
                            } else {
                                car_model_now.as_str()
                            };
                            let track_key = normalize_track_key(track_name);
                            let cum_def = cumulative_tracks.get(&track_key);
                            reset_live_timing_at_grid(
                                state,
                                cfg.timing_quality.physics_hz,
                                car_model,
                                cum_def,
                                &timing_event_bus,
                                &cfg.timing_reference_store_path,
                                cfg.reference_times.mode,
                            );
                            game_clock.reset();
                            pause_osd.reset();
                            grid_timing_reset_still_since = None;
                            latest_timing_line = None;
                            sector_status_line = Some((
                                format!(
                                    "timing reset at start ({:.0}s standstill)",
                                    START_GRID_TIMING_RESET_STILL_SEC
                                ),
                                Instant::now(),
                            ));
                            let _ = push_rtss_osd(cfg, "");
                            last_rtss_msg.clear();
                            last_rtss_push = Instant::now();
                        }
                    } else {
                        grid_timing_reset_still_since = None;
                    }
                } else {
                    grid_timing_reset_still_since = None;
                }
            } else {
                grid_timing_reset_still_since = None;
            }
            if locked_track.is_some() {
                if let Some(lp) = last_pt {
                    let jump_m = dist(lp, p);
                    if jump_m > START_LAYOUT_TELEPORT_RESET_M {
                        if let Some(state) = timing_state.as_mut() {
                            if state
                                .stage_sector_sessions
                                .iter()
                                .any(|s| s.run.armed && !s.run.completed)
                            {
                                note_stage_timing_position_reset(state);
                                note_subsection_timing_position_reset(state);
                            }
                        }
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
                            if let Some(state) = timing_state.as_mut() {
                                let car_model =
                                    locked_car_model.as_deref().unwrap_or("unknown_car");
                                flush_all_stage_sector_sessions(
                                    state,
                                    &cfg,
                                    &mut timing_pb,
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
                            grid_timing_reset_still_since = None;
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
                            let _ = push_rtss_osd(cfg, "");
                            last_rtss_msg.clear();
                            last_rtss_push = Instant::now();
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
                    let track_key = normalize_track_key(track_name);
                    if let Some(cum_def) = cumulative_tracks.get(&track_key) {
                        if timing_state.is_none() {
                            timing_state = Some(LiveTimingState::new(vec![]));
                        }
                        if let Some(state) = timing_state.as_mut() {
                            let car_model_live = {
                                let c = data.statics.car_model.trim();
                                if c.is_empty() {
                                    "unknown_car"
                                } else {
                                    c
                                }
                            };
                            if state.cumulative.is_none() {
                                state.run_clock = acr_timing::run_timing_clock::RunTimingClock::new(
                                    cfg.timing_quality.physics_hz,
                                );
                                state.cumulative = Some(
                                    acr_timing::cumulative_sector_timing::CumulativeLegState::new(
                                        cum_def.clone(),
                                    ),
                                );
                                ensure_modular_timing(
                                    state,
                                    &timing_event_bus,
                                    &cfg.timing_reference_store_path,
                                    cum_def,
                                    track_name,
                                    car_model_live,
                                    cfg.reference_times.mode,
                                );
                            }
                            ensure_stage_timing_sectors(
                                state,
                                track_name,
                                &cfg.stage_timing,
                                &mut timing_sector_cache,
                                &mut active_timing_stage_slug,
                                Some((p.x, p.z)),
                                Some(data.physics.heading),
                            );
                            if let Some(lp) = last_pt {
                                let now_inst = Instant::now();
                                let leg_cross = state
                                    .cumulative
                                    .as_mut()
                                    .and_then(|cum| {
                                        cum.observe_segment(
                                            (lp.x, lp.z),
                                            (p.x, p.z),
                                            total_drive_m,
                                            cfg.cumulative_timing.gate_radius_m(),
                                            Duration::from_millis(cfg.sector_cross_cooldown_ms),
                                            now_inst,
                                            cfg.timing_debug,
                                        )
                                    });
                                if let Some(leg) = leg_cross {
                                    let silent_cp = state
                                        .cumulative
                                        .as_ref()
                                        .is_some_and(|c| c.destination_is_silent_cp(leg.to_gate_ix));
                                    let pkt = data.physics.packet_id;
                                    let odo_m = data.graphics.distance_traveled as f64;
                                    if let Some((dt, dt_raw)) = compute_subsection_leg_dt(
                                        state,
                                        pkt,
                                        now_inst,
                                        &cfg.timing_quality,
                                        &game_clock,
                                    ) {
                                        let car_model =
                                            data.statics.car_model.trim();
                                        let car_model = if car_model.is_empty() {
                                            "unknown_car"
                                        } else {
                                            car_model
                                        };
                                        let exit_speed = data.physics.speed_kmh;
                                        let leg_stats =
                                            take_leg_stats(state, exit_speed);
                                        let outcome = commit_subsection_split(
                                            state,
                                            &timing_conn,
                                            &mut timing_pb,
                                            &cfg,
                                            track_name,
                                            car_model,
                                            "inc",
                                            leg.from_seg,
                                            leg.to_seg,
                                            dt,
                                            dt_raw,
                                            leg_distance_since_anchor(state, odo_m),
                                            leg_stats,
                                            locked_track.as_deref(),
                                            Some(&blame_ctx),
                                            true,
                                        );
                                        if cfg.timing_debug {
                                            let (from_label, to_label) = state
                                                .cumulative
                                                .as_ref()
                                                .map(|c| {
                                                    let m = &c.track.sectors.markers;
                                                    (
                                                        m.get(leg.from_gate_ix)
                                                            .map(|x| x.label.as_str())
                                                            .unwrap_or("?"),
                                                        m.get(leg.to_gate_ix)
                                                            .map(|x| x.label.as_str())
                                                            .unwrap_or("?"),
                                                    )
                                                })
                                                .unwrap_or(("?", "?"));
                                            let dbg = TimingDebugFrame {
                                                physics: &data.physics,
                                                graphics_x: p.x,
                                                graphics_z: p.z,
                                                graphics_current_time_ms: data.graphics.current_time,
                                                speed_kmh: data.physics.speed_kmh,
                                                distance_traveled_m: odo_m,
                                                packet_id: pkt,
                                            };
                                            acr_timing::timing_debug::log_subsektor_zeit(
                                                from_label,
                                                to_label,
                                                leg.from_seg,
                                                leg.to_seg,
                                                dt,
                                                dt_raw,
                                                state.subsection_cumulative_sec,
                                                dbg.run_sim_sec(state),
                                                dbg.spielzeit_sec(),
                                                &data.physics,
                                                p.x,
                                                p.z,
                                                data.physics.speed_kmh,
                                                odo_m,
                                                pkt,
                                            );
                                        }
                                        if let Some(m) = state.modular.as_mut() {
                                            m.coordinator.set_car(car_model);
                                            if silent_cp {
                                                m.coordinator.on_sub_cross(leg.to_seg, dt);
                                            } else if let Some(label) = state
                                                .cumulative
                                                .as_ref()
                                                .and_then(|c| {
                                                    c.track
                                                        .sectors
                                                        .markers
                                                        .get(leg.to_gate_ix)
                                                        .map(|m| m.label.as_str())
                                                })
                                            {
                                                if label.starts_with("Sector ") || label == "Finish"
                                                {
                                                    m.coordinator.on_sub_cross(leg.to_seg, dt);
                                                    m.coordinator
                                                        .on_main_sector_end(label, now_inst);
                                                }
                                            }
                                            let dbg = TimingDebugFrame {
                                                physics: &data.physics,
                                                graphics_x: p.x,
                                                graphics_z: p.z,
                                                graphics_current_time_ms: data.graphics.current_time,
                                                speed_kmh: data.physics.speed_kmh,
                                                distance_traveled_m: odo_m,
                                                packet_id: pkt,
                                            };
                                            let sync_brackets =
                                                if state.stage_sector_sessions.is_empty() {
                                                    None
                                                } else {
                                                    Some((&timing_pb, car_model_live))
                                                };
                                            drain_modular_timing_events(
                                                state,
                                                cfg,
                                                Some(&dbg),
                                                sync_brackets,
                                            );
                                        } else if silent_cp {
                                            // Beep only without modular presenter (else drain_modular handles it).
                                            if cfg.beep_on_cumulative_split
                                                && state.modular.is_none()
                                            {
                                                if let Some(d) = outcome.leg_pb_delta {
                                                    acr_timing::split_beep::play_split_feedback(
                                                        d,
                                                        &cfg.cumulative_beep,
                                                    );
                                                }
                                            }
                                            if let Some(d) = outcome.leg_pb_delta {
                                                sector_status_line = Some((
                                                    format!(
                                                        "CP [{from}]->[{to}] d{d:+.2}s",
                                                        from = leg.from_seg,
                                                        to = leg.to_seg,
                                                    ),
                                                    now_inst,
                                                ));
                                            }
                                        }
                                        state.run_clock.commit_leg(timing_anchor_now(
                                            pkt,
                                            odo_m,
                                            game_clock.game_race_for_sector_display(),
                                        ));
                                        reset_subsection_leg_timing_accumulators(state);
                                        state.leg_stats.reset();
                                        set_leg_entry_speed(state, exit_speed);
                                    }
                                } else if state.run_clock.leg_anchor().is_none() {
                                    if state
                                        .cumulative
                                        .as_ref()
                                        .is_some_and(|c| c.last_gate_is_timing_start())
                                    {
                                        let odo_m = data.graphics.distance_traveled as f64;
                                        state.run_clock.arm_run(timing_anchor_now(
                                            data.physics.packet_id,
                                            odo_m,
                                            game_clock.game_race_for_sector_display(),
                                        ));
                                        eprintln!("cumulative: timer anchored at Start (packet_id)");
                                        arm_modular_timing_run(state, cfg, car_model_live);
                                    }
                                }
                            }
                            if let Some(lp) = last_pt {
                                let step_m = dist(lp, p);
                                if step_m <= MAX_SECTOR_CROSS_SEGMENT_M {
                                    let car_model = data.statics.car_model.trim();
                                    let car_model = if car_model.is_empty() {
                                        "unknown_car"
                                    } else {
                                        car_model
                                    };
                                    process_stage_sector_sessions_on_step(
                                        state,
                                        lp,
                                        p,
                                        &cfg,
                                        &timing_conn,
                                        &mut timing_pb,
                                        timing_reference_store.as_ref(),
                                        &data.physics,
                                        data.physics.packet_id,
                                        cfg.timing_quality.physics_hz,
                                        data.graphics.distance_traveled,
                                        data.graphics.current_time,
                                        car_model,
                                        locked_track.as_deref(),
                                        &blame_ctx,
                                        &mut frame_monitor,
                                        &mut stage_sector_html_run_counter,
                                        speed_kmh_now,
                                        &mut game_clock_sector,
                                        &mut game_clock,
                                    );
                                }
                            }
                        }
                    } else if let Some(set) = sector_sets.get(track_name) {
                        if timing_state.is_none() {
                            timing_state = Some(LiveTimingState::new(set.ring_ids.clone()));
                            if let Some(state) = timing_state.as_mut() {
                                seed_sector_tracker_at_position(state, set, p);
                            }
                        }
                        if let Some(state) = timing_state.as_mut() {
                            let calibrated_stage_active =
                                !state.stage_sector_sessions.is_empty();
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
                                Some(data.physics.heading),
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
                                                let car_for_modular = if car_model_now.is_empty() {
                                                    "unknown_car"
                                                } else {
                                                    car_model_now.as_str()
                                                };
                                                arm_modular_timing_run(state, cfg, car_for_modular);
                                                reset_subsection_run(state);
                                                reset_subsection_leg_timing_accumulators(state);
                                                state.leg_stats.reset();
                                                set_leg_entry_speed(state, data.physics.speed_kmh);
                                                state.run_clock.arm_run(timing_anchor_now(
                                                    data.physics.packet_id,
                                                    data.graphics.distance_traveled as f64,
                                                    game_clock.game_race_for_sector_display(),
                                                ));
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
                                if state.run_clock.run_origin().is_none() {
                                    state.run_clock.arm_run(timing_anchor_now(
                                        data.physics.packet_id,
                                        data.graphics.distance_traveled as f64,
                                        game_clock.game_race_for_sector_display(),
                                    ));
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
                                            if let Some(origin) = state.run_clock.run_origin() {
                                                let pkt = data.physics.packet_id;
                                                let odo_m = data.graphics.distance_traveled as f64;
                                                if let Some(dt_raw) =
                                                    state.run_clock.run_sim_sec(pkt).filter(|t| *t > 0.05)
                                                {
                                                    let dt = finalize_subsection_split_dt(
                                                        state,
                                                        dt_raw,
                                                        &cfg.timing_quality,
                                                    );
                                                    let dist_m =
                                                        (odo_m - origin.distance_traveled_m).max(0.0);
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
                                                            distance_m: dist_m,
                                                            stats: leg_stats,
                                                        };
                                                    let (line, delta) =
                                                        if let Some(locked) =
                                                            locked_track.as_deref()
                                                        {
                                                            if locked == track_name {
                                                                persist_split_and_line(
                                                                    &timing_conn,
                                                                    &mut timing_pb,
                                                                    &split,
                                                                    Some(&blame_ctx),
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
                                    let car_model = data.statics.car_model.trim();
                                    let car_model = if car_model.is_empty() {
                                        "unknown_car"
                                    } else {
                                        car_model
                                    };
                                    process_stage_sector_sessions_on_step(
                                        state,
                                        lp,
                                        p,
                                        &cfg,
                                        &timing_conn,
                                        &mut timing_pb,
                                        timing_reference_store.as_ref(),
                                        &data.physics,
                                        data.physics.packet_id,
                                        cfg.timing_quality.physics_hz,
                                        data.graphics.distance_traveled,
                                        data.graphics.current_time,
                                        car_model,
                                        locked_track.as_deref(),
                                        &blame_ctx,
                                        &mut frame_monitor,
                                        &mut stage_sector_html_run_counter,
                                        speed_kmh_now,
                                        &mut game_clock_sector,
                                        &mut game_clock,
                                    );
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
                                                    if state.run_clock.run_origin().is_some() {
                                                        let pkt = data.physics.packet_id;
                                                        let now_inst = Instant::now();
                                                        let odo_m =
                                                            data.graphics.distance_traveled as f64;
                                                        if let Some((dt, dt_raw)) =
                                                            compute_subsection_leg_dt(
                                                                state,
                                                                pkt,
                                                                now_inst,
                                                                &cfg.timing_quality,
                                                                &game_clock,
                                                            )
                                                        {
                                                            let _ = dt_raw;
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
                                                            let outcome = commit_subsection_split(
                                                                state,
                                                                &timing_conn,
                                                                &mut timing_pb,
                                                                &cfg,
                                                                track_name,
                                                                car_model,
                                                                direction_s,
                                                                START_SECTOR_ID,
                                                                to_sector_id,
                                                                dt,
                                                                dt_raw,
                                                                leg_distance_since_anchor(
                                                                    state, odo_m,
                                                                ),
                                                                leg_stats,
                                                                locked_track.as_deref(),
                                                                Some(&blame_ctx),
                                                                false,
                                                            );
                                                            eprintln!("{}", outcome.line);
                                                            if !calibrated_stage_active {
                                                                latest_timing_line = Some((
                                                                    outcome.line.clone(),
                                                                    Instant::now(),
                                                                ));
                                                            }
                                                            if cfg.beep_on_split && outcome.persisted {
                                                                acr_timing::split_beep::play_split_feedback(
                                                                    outcome.leg_delta,
                                                                    &cfg.split_beep,
                                                                );
                                                            }
                                                        }
                                                    }
                                                    state.start_armed = false;
                                                    state.start_stage_pos = None;
                                                    state.start_stage_since = None;
                                                    state.start_stage_last_report_sec = -1;
                                                }
                                                reset_subsection_leg_timing_accumulators(state);
                                                state.run_clock.commit_leg(timing_anchor_now(
                                                    data.physics.packet_id,
                                                    data.graphics.distance_traveled as f64,
                                                    game_clock.game_race_for_sector_display(),
                                                ));
                                                state.last_sector_idx = Some(sector);
                                                state.leg_stats.reset();
                                                set_leg_entry_speed(state, data.physics.speed_kmh);
                                                let anchor_line = format!(
                                                    "sector [{}]...",
                                                    state.ring_ids[sector]
                                                );
                                                eprintln!("{}", anchor_line);
                                                if !calibrated_stage_active {
                                                    sector_status_line =
                                                        Some((anchor_line, Instant::now()));
                                                }
                                            }
                                            SectorPassEvent::Step { from, to, direction } => {
                                                let now_inst = Instant::now();
                                                let pkt = data.physics.packet_id;
                                                let odo_m =
                                                    data.graphics.distance_traveled as f64;
                                                if state.run_clock.leg_anchor().is_some() {
                                                    if let Some((dt, dt_raw)) =
                                                        compute_subsection_leg_dt(
                                                            state,
                                                            pkt,
                                                            now_inst,
                                                            &cfg.timing_quality,
                                                            &game_clock,
                                                        )
                                                    {
                                                        let dist_m =
                                                            leg_distance_since_anchor(state, odo_m);
                                                        let _ = dt_raw;
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
                                                        let outcome = commit_subsection_split(
                                                            state,
                                                            &timing_conn,
                                                            &mut timing_pb,
                                                            &cfg,
                                                            track_name,
                                                            car_model,
                                                            direction_s,
                                                            from_sector_id,
                                                            to_sector_id,
                                                            dt,
                                                            dt_raw,
                                                            dist_m,
                                                            leg_stats,
                                                            locked_track.as_deref(),
                                                            Some(&blame_ctx),
                                                            false,
                                                        );
                                                        eprintln!("{}", outcome.line);
                                                        if !calibrated_stage_active {
                                                            latest_timing_line = Some((
                                                                outcome.line.clone(),
                                                                Instant::now(),
                                                            ));
                                                        }
                                                        if cfg.beep_on_split && outcome.persisted {
                                                            acr_timing::split_beep::play_split_feedback(
                                                                outcome.leg_delta,
                                                                &cfg.split_beep,
                                                            );
                                                        }
                                                    }
                                                }
                                                eprintln!(
                                                    "sector passed [{}]",
                                                    state.ring_ids[to]
                                                );
                                                if active_track_name.is_some()
                                                    && !calibrated_stage_active
                                                {
                                                    let passed_line = format!(
                                                        "sector passed [{}]",
                                                        state.ring_ids[to]
                                                    );
                                                    sector_status_line =
                                                        Some((passed_line.clone(), Instant::now()));
                                                }
                                                state.run_clock.commit_leg(timing_anchor_now(
                                                    pkt,
                                                    odo_m,
                                                    game_clock.game_race_for_sector_display(),
                                                ));
                                                state.last_sector_idx = Some(to);
                                                set_leg_entry_speed(state, data.physics.speed_kmh);
                                            }
                                            SectorPassEvent::NoStep { .. }
                                            => {
                                                // Typical restart case: same sector crossed again after a pause.
                                                // Re-anchor timing to avoid carrying over a stale start timestamp.
                                                let now_inst2 = Instant::now();
                                                let should_reanchor = state
                                                    .run_clock
                                                    .leg_anchor()
                                                    .map(|a| {
                                                        now_inst2.duration_since(a.at).as_secs_f64()
                                                            >= SAME_SECTOR_REANCHOR_SEC
                                                    })
                                                    .unwrap_or(true);
                                                if should_reanchor {
                                                    reset_subsection_leg_timing_accumulators(state);
                                                    state.run_clock.commit_leg(timing_anchor_now(
                                                        data.physics.packet_id,
                                                        data.graphics.distance_traveled as f64,
                                                        game_clock.game_race_for_sector_display(),
                                                    ));
                                                    state.leg_stats.reset();
                                                    set_leg_entry_speed(state, data.physics.speed_kmh);
                                                    if let Some(si) = state.last_sector_idx {
                                                        let line = format!("sector [{}]...", state.ring_ids[si]);
                                                        eprintln!("re-anchored at same sector: {}", line);
                                                        if !calibrated_stage_active {
                                                            sector_status_line =
                                                                Some((line, Instant::now()));
                                                        }
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
                                    Some(data.physics.heading),
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
                            Some(data.physics.heading),
                        );
                    }
                    if last_sector_wait_log.elapsed() >= Duration::from_secs(5) {
                        eprintln!("track locked: {}", locked_name);
                        last_sector_wait_log = Instant::now();
                    }
                    let now_osd = Instant::now();
                    let game_race_s = game_clock.game_race_for_sector_display();
                    let pause_dash = game_clock.osd_show_pause_dash(&mut pause_osd);
                    let msg = if let Some(ts) = timing_state.as_mut() {
                        let car_osd = locked_car_model
                            .as_deref()
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| {
                                if car_model_now.is_empty() {
                                    "unknown_car"
                                } else {
                                    car_model_now.as_str()
                                }
                            });
                        build_rtss_timing_message(
                            ts,
                            cfg,
                            &timing_pb,
                            car_osd,
                            now_osd,
                            game_race_s,
                            pause_dash,
                            &mut pause_osd,
                            &sector_status_line,
                        )
                    } else {
                        String::new()
                    };
                    let osd_push_interval = if timing_state.as_ref().is_some_and(|s| {
                        !s.stage_sector_sessions.is_empty() || s.cumulative.is_some()
                    }) {
                        Duration::from_millis(400)
                    } else {
                        Duration::from_secs(2)
                    };
                    if !msg.is_empty()
                        && (msg != last_rtss_msg || last_rtss_push.elapsed() >= osd_push_interval)
                    {
                        push_rtss_osd(cfg, &msg)?;
                        last_rtss_msg = msg;
                        last_rtss_push = Instant::now();
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
                                let mut ts = LiveTimingState::new_preserving_cumulative(
                                    s.ring_ids.clone(),
                                    timing_state.take(),
                                    &selected.name,
                                );
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
                    if best.coarse_pass && timing_state.is_some() && latest_timing_line.is_none() {
                        if last_sector_wait_log.elapsed() >= Duration::from_secs(3) {
                            eprintln!("waiting for sector passing...");
                            last_sector_wait_log = Instant::now();
                        }
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
                push_rtss_osd(cfg, &msg)?;
                last_rtss_msg = msg;
                last_rtss_push = Instant::now();
            } else if let Some(ref amb) = pacenote_ambiguous_pick {
                let msg = pacenote_amb_overlay::build_overlay_text(amb);
                push_rtss_osd(cfg, &msg)?;
                last_rtss_msg = msg;
                last_rtss_push = Instant::now();
            } else if locked_track.is_none() {
                let msg = build_rtss_pre_lock_message(cfg);
                if !msg.is_empty()
                    && (msg != last_rtss_msg || last_rtss_push.elapsed() >= Duration::from_secs(2))
                {
                    push_rtss_osd(cfg, &msg)?;
                    last_rtss_msg = msg;
                    last_rtss_push = Instant::now();
                } else if msg.is_empty()
                    && last_rtss_msg == acr_timing::minimal_osd::GAME_DATA_AVAILABLE_TEXT
                {
                    let _ = push_rtss_osd(cfg, "");
                    last_rtss_msg.clear();
                    last_rtss_push = Instant::now();
                }
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
    pb: &mut acr_timing::timing_pb::TimingPbStore,
    split: &acr_timing::timing_db::SplitRecord<'_>,
    blame_ctx: Option<&TimingBlameCtx<'_>>,
) -> (String, f64) {
    // Compare against PB *before* insert (timing_pb.toml); archive every run in timing.db.
    let best_before = pb.best_before_and_maybe_update(split).ok().flatten();
    let _ = acr_timing::timing_db::insert_split(conn, split);
    let delta = best_before
        .map(|b| split.duration_sec - b)
        .unwrap_or(0.0);
    if let Some(pb) = best_before {
        if delta > blame_ctx.map(|c| c.cfg.min_delta_sec).unwrap_or(0.05) {
            maybe_timing_blame(conn, split, pb, delta, blame_ctx);
        }
    }
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

fn push_rtss_osd(cfg: &CliConfig, msg: &str) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(windows)]
    {
        if cfg.rtss {
            let safe = acr_timing::rtss_osd::sanitize_multiline_osd_text(
                msg,
                acr_timing::rtss_osd::DEFAULT_MAX_OSD_LINES,
            );
            let osd = cfg.rtss_osd_placement.apply_to_text(&safe);
            if let Err(e) = acr_timing::rtss_osd::update(&cfg.rtss_owner, &osd, cfg.rtss_slot) {
                eprintln!("RTSS update failed: {}", e);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (cfg, msg);
    Ok(())
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
