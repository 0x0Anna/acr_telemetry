//! Read `acr_game_clock.jsonl` from external timing provider (ACR race time + distance) and optionally
//! steer a replica game clock toward those samples between ticks.
//!
//! ACR does not expose rally time via ACC shared memory; the mod writes game-internal
//! `RaceStateData.RaceTime` and `DistanceOnMainSpline`.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use chrono::{Local, TimeZone};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct GameClockSectorRecord {
    pub id: Option<i32>,
    pub time_s: Option<f64>,
    pub split_s: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameClockGhostRef {
    #[serde(default)]
    pub source: Option<String>,
    pub diff_time_s: Option<f64>,
    pub penalty_total_s: Option<f64>,
    #[serde(default)]
    pub sectors: Vec<GameClockSectorRecord>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameClockSectorsDebug {
    pub array_num: Option<i32>,
    pub parsed: Option<i32>,
    pub first_err: Option<String>,
    pub err: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameClockSample {
    pub race_time_s: Option<f64>,
    pub distance_m: Option<f64>,
    #[serde(default)]
    pub race_time_valid: bool,
    pub diff_time_s: Option<f64>,
    pub position: Option<i32>,
    pub phase: Option<i32>,
    #[serde(default)]
    pub sectors: Vec<GameClockSectorRecord>,
    pub sectors_source: Option<String>,
    pub sectors_debug: Option<GameClockSectorsDebug>,
    pub next_sector_index: Option<i32>,
    /// Which UObject path supplied `RaceStateData` (debug).
    pub race_source: Option<String>,
    /// `AcrGameState.TravelTrackId` (stage/track id for this session).
    pub travel_track_id: Option<String>,
    pub travel_track_source: Option<String>,
    pub penalty_total_s: Option<f64>,
    pub ghost_ref: Option<GameClockGhostRef>,
    pub game_x: Option<f64>,
    pub game_z: Option<f64>,
    #[serde(default)]
    pub t_process_ms: Option<u64>,
    /// external timing provider mod wall clock (unix seconds) when the line was written.
    #[serde(default)]
    pub t_wall_s: Option<i64>,
    /// `"light"` = race time only; `"full"` includes sectors/ghost (external timing provider mod).
    #[serde(rename = "sample", default)]
    pub sample_kind: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GameClockSyncConfig {
    pub enabled: bool,
    /// JSONL path; empty = `%APPDATA%/acr_telemetry/acr_game_clock.jsonl` on Windows.
    pub jsonl_path: PathBuf,
    /// Ignore samples older than this (seconds, by file mtime).
    pub max_sample_age_sec: f64,
    /// Expected external timing provider interval for rate correction (seconds).
    pub expected_tick_sec: f64,
    /// Max per-tick rate adjustment (e.g. 0.02 = ±2% sim-time speed).
    pub max_rate_adjust: f64,
    /// Also nudge replica distance toward game `distance_m`.
    pub correct_distance: bool,
    /// Min seconds between JSONL tail reads (lower = more CPU/disk, can hitch).
    pub jsonl_poll_interval_sec: f64,
    /// Running: snap replica to game when |error| exceeds this; else nudge `rate_time` per sample.
    pub time_snap_threshold_sec: f64,
    /// Fraction of (game − replica) applied as a soft snap on each sample (0 = rate only).
    pub time_soft_snap_blend: f64,
}

impl Default for GameClockSyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            jsonl_path: default_jsonl_path(),
            max_sample_age_sec: 1.25,
            expected_tick_sec: 0.5,
            max_rate_adjust: 0.02,
            correct_distance: true,
            jsonl_poll_interval_sec: 0.5,
            time_snap_threshold_sec: 0.4,
            time_soft_snap_blend: 0.35,
        }
    }
}

fn is_light_sample(s: &GameClockSample) -> bool {
    s.sample_kind.as_deref() == Some("light")
}

/// Keep sector/ghost from the last full external timing provider line when a light tick omits them.
pub fn merge_game_clock_sample(prev: Option<GameClockSample>, mut new: GameClockSample) -> GameClockSample {
    let Some(p) = prev else {
        return new;
    };
    if !is_light_sample(&new) {
        return new;
    }
    if new.sectors.is_empty() {
        new.sectors = p.sectors;
        if new.sectors_source.is_none() {
            new.sectors_source = p.sectors_source;
        }
        if new.sectors_debug.is_none() {
            new.sectors_debug = p.sectors_debug;
        }
        if new.next_sector_index.is_none() {
            new.next_sector_index = p.next_sector_index;
        }
    }
    if new.ghost_ref.is_none() {
        new.ghost_ref = p.ghost_ref;
    }
    new
}

pub fn default_jsonl_path() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("acr_telemetry").join("acr_game_clock.jsonl");
    }
    PathBuf::from("acr_game_clock.jsonl")
}

/// Tail-read the newest valid JSON line from the mod output file.
pub fn read_latest_sample(path: &Path, max_age_sec: f64) -> Option<(GameClockSample, Instant)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let read_at = Instant::now();
    let age = SystemTime::now().duration_since(modified).ok()?.as_secs_f64();
    if age > max_age_sec {
        return None;
    }
    let line = read_last_line(path)?;
    let sample: GameClockSample = serde_json::from_str(&line).ok()?;
    if sample.race_time_s.is_none() && sample.distance_m.is_none() {
        return None;
    }
    Some((sample, read_at))
}

fn read_last_line(path: &Path) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    let scan = len.min(16 * 1024);
    let start = len.saturating_sub(scan);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    text.lines()
        .rev()
        .find_map(|l| {
            let t = l.trim();
            if t.starts_with('{') {
                Some(t.to_string())
            } else {
                None
            }
        })
}

/// Replica of game race clock advanced with physics `dt_sim`, steered toward external timing provider samples.
///
/// Model: each frame `replica += dt_sim * rate_time` while the game clock runs; on each JSONL
/// sample compare game HUD `race_time_s` to replica — large gap → snap, small gap → adjust
/// `rate_time` (and optional soft snap). When `race_time_valid` is false (pause), replica
/// freezes at the last game time and `rate_time` is 0.
#[derive(Debug, Clone)]
pub struct GameClockCorrector {
    cfg: GameClockSyncConfig,
    replica_time_sec: f64,
    replica_distance_m: f64,
    rate_time: f64,
    rate_distance: f64,
    last_sample: Option<GameClockSample>,
    last_sample_read: Option<Instant>,
    last_file_poll: Instant,
    last_our_distance_m: Option<f64>,
    initialized: bool,
    last_sync_debug_log: Instant,
    /// Wall clock when [`apply_sample`] last ingested JSONL.
    last_apply_wall: Option<SystemTime>,
    last_apply_game_t: Option<f64>,
    last_apply_replica_before: Option<f64>,
    last_apply_err_sec: Option<f64>,
    /// `race_time_s` at JSONL ingest, advanced by file age (game time when line was written).
    last_ingest_game_t: Option<f64>,
}

impl GameClockCorrector {
    pub fn new(cfg: GameClockSyncConfig) -> Self {
        Self {
            cfg,
            replica_time_sec: 0.0,
            replica_distance_m: 0.0,
            rate_time: 1.0,
            rate_distance: 1.0,
            last_sample: None,
            last_sample_read: None,
            last_file_poll: Instant::now(),
            last_our_distance_m: None,
            initialized: false,
            last_sync_debug_log: Instant::now()
                .checked_sub(Duration::from_secs(3600))
                .unwrap_or_else(Instant::now),
            last_apply_wall: None,
            last_apply_game_t: None,
            last_apply_replica_before: None,
            last_apply_err_sec: None,
            last_ingest_game_t: None,
        }
    }

    fn jsonl_file_age_sec(path: &Path) -> Option<f64> {
        let modified = std::fs::metadata(path).ok()?.modified().ok()?;
        SystemTime::now()
            .duration_since(modified)
            .ok()
            .map(|d| d.as_secs_f64())
    }

    /// `race_time_s` at ingest + wall time since that JSONL read (covers poll/mod delay).
    fn game_race_wall_extrapolated_sec(&self) -> Option<f64> {
        let read_at = self.last_sample_read?;
        let stale = read_at.elapsed().as_secs_f64();
        let rate = if self.race_time_running() {
            self.rate_time.max(0.0)
        } else {
            0.0
        };
        if let Some(t0) = self.last_ingest_game_t {
            return Some(t0 + stale.min(2.0) * rate);
        }
        let base = self.game_race_live_sec()?;
        let file_age = Self::jsonl_file_age_sec(&self.cfg.jsonl_path).unwrap_or(0.0);
        Some(base + (stale + file_age).min(2.0) * rate)
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub fn replica_race_time_sec(&self) -> Option<f64> {
        if self.cfg.enabled && self.initialized {
            Some(self.replica_time_sec)
        } else {
            None
        }
    }

    /// HUD rally time: wall extrapolation from last JSONL ingest while running (not physics dt only).
    pub fn game_race_hud_sec(&self) -> Option<f64> {
        if !self.cfg.enabled {
            return None;
        }
        if self.race_time_running() {
            return self
                .game_race_wall_extrapolated_sec()
                .or_else(|| self.replica_race_time_sec());
        }
        self.game_race_live_sec()
            .or_else(|| self.replica_race_time_sec())
    }

    /// Read JSONL immediately (e.g. right after a sector gate).
    pub fn poll_now(&mut self, our_distance_m: Option<f64>) {
        if !self.cfg.enabled {
            return;
        }
        if let Some((sample, read_at)) =
            read_latest_sample(&self.cfg.jsonl_path, self.cfg.max_sample_age_sec)
        {
            self.apply_sample(sample, read_at, our_distance_m);
        }
    }

    /// Rally time from the last fresh JSONL sample (not extrapolated).
    pub fn game_race_live_sec(&self) -> Option<f64> {
        if !self.cfg.enabled {
            return None;
        }
        let read_at = self.last_sample_read?;
        if read_at.elapsed().as_secs_f64() > self.cfg.max_sample_age_sec {
            return None;
        }
        let sample = self.last_sample.as_ref()?;
        sample.race_time_s.filter(|t| t.is_finite() && *t >= 0.0)
    }

    /// When false, game HUD time is frozen (pause/menu) — do not advance replica from physics dt.
    pub fn race_time_running(&self) -> bool {
        self.last_sample
            .as_ref()
            .map(|s| s.race_time_valid)
            .unwrap_or(false)
    }

    pub fn replica_distance_m(&self) -> Option<f64> {
        if self.cfg.enabled && self.initialized {
            Some(self.replica_distance_m)
        } else {
            None
        }
    }

    pub fn last_game_sample(&self) -> Option<&GameClockSample> {
        self.last_sample.as_ref()
    }

    pub fn time_rate(&self) -> f64 {
        self.rate_time
    }

    /// Advance replica; poll jsonl at most ~10 Hz; apply rate correction when a fresh sample arrives.
    pub fn tick(&mut self, dt_sim_sec: f64, our_distance_m: Option<f64>) {
        if !self.cfg.enabled || dt_sim_sec <= 0.0 {
            return;
        }

        let poll_iv = self.cfg.jsonl_poll_interval_sec.max(0.2);
        if self.last_file_poll.elapsed().as_secs_f64() >= poll_iv {
            self.last_file_poll = Instant::now();
            if let Some((sample, read_at)) =
                read_latest_sample(&self.cfg.jsonl_path, self.cfg.max_sample_age_sec)
            {
                self.apply_sample(sample, read_at, our_distance_m);
            }
        }

        if self.race_time_running() && self.rate_time > 0.0 {
            if let Some(hud) = self.game_race_wall_extrapolated_sec() {
                self.replica_time_sec = hud;
            } else {
                self.replica_time_sec += dt_sim_sec * self.rate_time;
            }
        }
        if let Some(d) = our_distance_m {
            if let Some(prev) = self.last_our_distance_m {
                let delta = (d - prev).max(0.0);
                self.replica_distance_m += delta * self.rate_distance;
            } else if !self.initialized {
                self.replica_distance_m = d;
            }
            self.last_our_distance_m = Some(d);
        }
    }

    fn apply_sample(
        &mut self,
        sample: GameClockSample,
        read_at: Instant,
        our_distance_m: Option<f64>,
    ) {
        let horizon = self.cfg.expected_tick_sec.max(0.25);

        let file_age = Self::jsonl_file_age_sec(&self.cfg.jsonl_path)
            .filter(|a| *a < self.cfg.max_sample_age_sec)
            .unwrap_or(0.0)
            .min(0.5);
        if let Some(game_t) = sample.race_time_s.filter(|t| t.is_finite()) {
            let ingest_rate = if sample.race_time_valid { 1.0 } else { 0.0 };
            let ingest_game = game_t + file_age * ingest_rate;
            self.last_ingest_game_t = Some(ingest_game);
            if !self.initialized {
                self.replica_time_sec = ingest_game;
                self.rate_time = ingest_rate;
                self.initialized = true;
            } else if !sample.race_time_valid {
                // Game frozen (pause/menu): hold HUD time, do not advance on physics ticks.
                self.replica_time_sec = ingest_game;
                self.rate_time = 0.0;
            } else {
                // 1-Hz-Zeitkorrektur: Replik läuft weiter (physics tick), JSONL zieht nach.
                let replica_before = self.replica_time_sec;
                let err = ingest_game - replica_before;
                self.last_apply_wall = Some(SystemTime::now());
                self.last_apply_game_t = Some(ingest_game);
                self.last_apply_replica_before = Some(replica_before);
                self.last_apply_err_sec = Some(err);
                let snap = self.cfg.time_snap_threshold_sec.max(0.05);
                if err.abs() > snap {
                    self.replica_time_sec = ingest_game;
                    self.rate_time = 1.0;
                } else if err.abs() > 1e-4 {
                    let horizon = self.cfg.expected_tick_sec.max(0.25);
                    let blend = self.cfg.time_soft_snap_blend.clamp(0.0, 1.0);
                    if blend > 0.0 {
                        self.replica_time_sec += err * blend;
                    }
                    self.rate_time = (1.0 + err / horizon).clamp(
                        1.0 - self.cfg.max_rate_adjust,
                        1.0 + self.cfg.max_rate_adjust,
                    );
                } else {
                    self.rate_time = 1.0;
                }
            }
        }

        if let Some(game_d) = sample.distance_m {
            if !self.initialized {
                self.replica_distance_m = game_d;
            } else if self.cfg.correct_distance {
                let err = game_d - self.replica_distance_m;
                self.rate_distance = (1.0 + err / horizon)
                    .clamp(1.0 - self.cfg.max_rate_adjust, 1.0 + self.cfg.max_rate_adjust);
            } else {
                self.replica_distance_m = game_d;
            }
            if sample.race_time_s.is_some() {
                self.initialized = true;
            }
        } else if let Some(d) = our_distance_m {
            if !self.initialized {
                self.replica_distance_m = d;
                self.initialized = true;
            }
        }

        let merged = merge_game_clock_sample(self.last_sample.take(), sample);
        self.last_sample = Some(merged);
        self.last_sample_read = Some(read_at);
    }

    /// Clear replica (grid reset / new stage).
    pub fn reset(&mut self) {
        self.replica_time_sec = 0.0;
        self.replica_distance_m = 0.0;
        self.rate_time = 1.0;
        self.rate_distance = 1.0;
        self.initialized = false;
        self.last_sample = None;
        self.last_sample_read = None;
        self.last_our_distance_m = None;
        self.last_apply_wall = None;
        self.last_apply_game_t = None;
        self.last_apply_replica_before = None;
        self.last_apply_err_sec = None;
        self.last_ingest_game_t = None;
    }

    /// JSONL sample still within `[game_clock] max_sample_age_sec`.
    pub fn jsonl_fresh_for_display(&self) -> bool {
        self.last_sample_read
            .map(|t| t.elapsed().as_secs_f64() < self.cfg.max_sample_age_sec)
            .unwrap_or(false)
    }

    /// JSONL considered live for pause OSD (see `TIMING_OPERATING_MODES.md` §6).
    pub fn jsonl_fresh_for_pause_osd(&self) -> bool {
        const MAX_AGE_SEC: f64 = 2.0;
        self.last_sample_read
            .map(|t| t.elapsed().as_secs_f64() < MAX_AGE_SEC)
            .unwrap_or(false)
    }

    /// Replik-Spielzeit für Live-Sektor und Gate-Math (läuft zwischen JSONL-Samples weiter).
    ///
    /// JSONL korrigiert die Replik via [`tick`] / [`apply_sample`] (1-Hz-Zeitkorrektur), ersetzt
    /// die Anzeige nicht im Poll-Takt.
    pub fn game_race_for_sector_display(&self) -> Option<f64> {
        if !self.cfg.enabled || !self.initialized || !self.jsonl_fresh_for_display() {
            return None;
        }
        self.game_race_hud_sec()
    }

    /// `race_time_s − replica` (positiv = Replik hinter Spiel-HUD).
    pub fn replica_lag_vs_game_sec(&self) -> Option<f64> {
        match (self.game_race_live_sec(), self.replica_race_time_sec()) {
            (Some(game), Some(rep)) => Some(game - rep),
            _ => None,
        }
    }

    fn fmt_wall(t: SystemTime) -> String {
        match t.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => Local
                .timestamp_opt(d.as_secs() as i64, d.subsec_nanos())
                .single()
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
                .unwrap_or_else(|| "?".into()),
            Err(_) => "?".into(),
        }
    }

    /// 1 Hz stderr line for `timing_debug`: system clock, last JSONL ingest, game vs replica.
    pub fn maybe_log_sync_debug(&mut self, stage_anchor_sec: Option<f64>) {
        if !self.cfg.enabled || !self.initialized {
            return;
        }
        if self.last_sync_debug_log.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_sync_debug_log = Instant::now();

        let wall_now = SystemTime::now();
        let wall_s = Self::fmt_wall(wall_now);

        let jsonl_read_ago = self
            .last_sample_read
            .map(|t| t.elapsed().as_secs_f64());
        let jsonl_file_age = Self::jsonl_file_age_sec(&self.cfg.jsonl_path);
        let sample = self.last_sample.as_ref();
        let jsonl_race = sample.and_then(|s| s.race_time_s);
        let sample_kind = sample
            .and_then(|s| s.sample_kind.as_deref())
            .unwrap_or("-");
        let valid = sample.map(|s| s.race_time_valid).unwrap_or(false);
        let mod_wall = sample.and_then(|s| s.t_wall_s);

        let hud = self.game_race_hud_sec();
        let wall_hud = self.game_race_wall_extrapolated_sec();
        let lag = self.replica_lag_vs_game_sec();
        let rel_hud = stage_anchor_sec.and_then(|a| hud.map(|h| (h - a).max(0.0)));

        let sync_wall = self
            .last_apply_wall
            .map(Self::fmt_wall)
            .unwrap_or_else(|| "-".into());
        let sync_ago = self
            .last_apply_wall
            .and_then(|w| wall_now.duration_since(w).ok())
            .map(|d| d.as_secs_f64());
        let sync_ago_s = sync_ago
            .map(|a| format!("{a:.1}"))
            .unwrap_or_else(|| "-".into());
        let mod_wall_s = mod_wall
            .map(|t| format!("{t}"))
            .unwrap_or_else(|| "-".into());
        let corr = match (
            self.last_apply_err_sec,
            self.last_apply_game_t,
            self.last_apply_replica_before,
        ) {
            (Some(err), Some(g), Some(r0)) => format!(
                "game−repl_vor={err:+.3}s (game={g:.3}s repl_vor={r0:.3}s)"
            ),
            _ => "keine Korrektur seit Start".into(),
        };

        eprintln!(
            "[game_clock_sync] Aktuelle Systemzeit: {wall_s} | \
letzte JSONL-Synchro (System): {sync_wall} vor {sync_ago_s}s | \
JSONL-Inhalt: race_time_s={} sample={sample_kind} valid={valid} mod_t_wall_s={mod_wall_s} file_age={}s read_ago={}s sectors={} | \
Spielzeit: JSONL_race_t={} HUD_wall={} (jsonl+stale+file_age) Replik={} lag(JSONL−repl)={} rate={:.4} poll_iv={:.2}s | \
stage_rel_hud={} (anchor={}) | letzte Korrektur: {corr}",
            jsonl_race
                .map(|t| format!("{t:.3}"))
                .unwrap_or_else(|| "-".into()),
            jsonl_file_age
                .map(|a| format!("{a:.2}"))
                .unwrap_or_else(|| "-".into()),
            jsonl_read_ago
                .map(|a| format!("{a:.2}"))
                .unwrap_or_else(|| "-".into()),
            sample.map(|s| s.sectors.len()).unwrap_or(0),
            jsonl_race
                .map(|t| format!("{t:.3}"))
                .unwrap_or_else(|| "-".into()),
            wall_hud
                .map(|t| format!("{t:.3}"))
                .unwrap_or_else(|| "-".into()),
            self.replica_race_time_sec()
                .map(|t| format!("{t:.3}"))
                .unwrap_or_else(|| "-".into()),
            lag.map(|l| format!("{l:+.3}s"))
                .unwrap_or_else(|| "-".into()),
            self.rate_time,
            self.cfg.jsonl_poll_interval_sec,
            rel_hud
                .map(|t| format!("{t:.3}s"))
                .unwrap_or_else(|| "-".into()),
            stage_anchor_sec
                .map(|a| format!("{a:.3}"))
                .unwrap_or_else(|| "-".into()),
        );
    }

    /// Do not advance subsection/stage timing or stall-excess while game HUD time is frozen.
    pub fn timing_frozen(&self) -> bool {
        if !self.cfg.enabled || !self.initialized || !self.jsonl_fresh_for_pause_osd() {
            return false;
        }
        let live = self.game_race_live_sec();
        let diverged = match (self.replica_race_time_sec(), live) {
            (Some(rep), Some(game)) => (rep - game).abs() > OSD_PAUSE_DIVERGE_SEC,
            _ => false,
        };
        !self.race_time_running() || diverged
    }
}

/// Pause handling for the middle cumulative-Δ RTSS line (`--` until resume).
#[derive(Debug, Default)]
pub struct PauseOsdState {
    latched: bool,
    last_hud_sec: Option<f64>,
    resume_ticks: u8,
    /// Cumulative Δ frozen when pause OSD latches (display only).
    pub frozen_cum_delta_sec: Option<f64>,
}

impl PauseOsdState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

const OSD_PAUSE_DIVERGE_SEC: f64 = 1.0;
const OSD_RESUME_MAX_STEP_SEC: f64 = 1.0;

impl GameClockCorrector {
    /// Whether the big cumulative-Δ line should show `--` (pause / replica vs game diverged).
    pub fn osd_show_pause_dash(&self, pause: &mut PauseOsdState) -> bool {
        if !self.cfg.enabled || !self.initialized {
            pause.reset();
            return false;
        }
        if !self.jsonl_fresh_for_pause_osd() {
            pause.reset();
            return false;
        }

        let hud = self.game_race_hud_sec();
        let live = self.game_race_live_sec();
        let diverged = match (self.replica_race_time_sec(), live) {
            (Some(rep), Some(game)) => (rep - game).abs() > OSD_PAUSE_DIVERGE_SEC,
            _ => false,
        };
        let paused_now = !self.race_time_running() || diverged;

        if paused_now {
            pause.latched = true;
            pause.resume_ticks = 0;
            pause.last_hud_sec = hud;
            return true;
        }

        if !pause.latched {
            pause.last_hud_sec = hud;
            return false;
        }

        if let (Some(prev), Some(cur)) = (pause.last_hud_sec, hud) {
            let dt = cur - prev;
            if dt > 0.0 && dt < OSD_RESUME_MAX_STEP_SEC {
                pause.resume_ticks = pause.resume_ticks.saturating_add(1);
            } else {
                pause.resume_ticks = 0;
            }
        } else if hud.is_some() {
            pause.resume_ticks = 1;
        }
        pause.last_hud_sec = hud;

        if pause.resume_ticks >= 2 {
            pause.latched = false;
            pause.resume_ticks = 0;
            false
        } else {
            true
        }
    }
}

/// Last JSON object line + file age (seconds), even when older than `max_sample_age_sec`.
pub fn read_last_sample_with_age(path: &Path) -> Option<(GameClockSample, f64)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?.as_secs_f64();
    let line = read_last_line(path)?;
    let sample: GameClockSample = serde_json::from_str(&line).ok()?;
    Some((sample, age))
}

/// Options for the RTSS demo overlay formatter.
#[derive(Debug, Clone, Copy, Default)]
pub struct RtssDemoFormatOpts {
    /// Append last JSONL line (large; can make RTSS stutter). Default off in demo binary.
    pub include_raw_json: bool,
}

/// Multi-line RTSS dump of all game-clock fields (compact; no raw JSON unless requested).
pub fn format_rtss_demo_text(
    sample: &GameClockSample,
    file_age_sec: f64,
    jsonl_path: &Path,
    opts: RtssDemoFormatOpts,
) -> String {
    let mut lines = Vec::new();
    lines.push("=== ACR game_clock ===".to_string());
    lines.push(format!("jsonl_age_s: {file_age_sec:.1}"));
    if file_age_sec > 2.5 {
        lines.push("WARN stale jsonl (>2.5s) — mod running?".to_string());
    }
    lines.push(String::new());

    lines.push("-- track --".to_string());
    lines.push(format!(
        "travel_track_id: {}",
        sample
            .travel_track_id
            .as_deref()
            .unwrap_or("-")
    ));
    if let Some(src) = &sample.travel_track_source {
        lines.push(format!("travel_track_source: {src}"));
    }
    lines.push(String::new());

    lines.push("-- race (RaceStateData) --".to_string());
    lines.push(format!(
        "race_time_s: {}",
        fmt_opt_f64(sample.race_time_s, 3)
    ));
    lines.push(format!(
        "distance_m: {}",
        fmt_opt_f64(sample.distance_m, 1)
    ));
    lines.push(format!("race_time_valid: {}", sample.race_time_valid));
    lines.push(format!(
        "phase: {}",
        sample
            .phase
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into())
    ));
    lines.push(format!("diff_time_s: {}", fmt_opt_f64(sample.diff_time_s, 3)));
    lines.push(format!(
        "position: {}",
        sample
            .position
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into())
    ));
    lines.push(format!(
        "penalty_total_s: {}",
        fmt_opt_f64(sample.penalty_total_s, 3)
    ));
    if let Some(src) = &sample.race_source {
        lines.push(format!("race_source: {src}"));
    }
    if let Some(n) = sample.next_sector_index {
        lines.push(format!("next_sector_index: {n}"));
    }
    lines.push(format!(
        "game_x/z: {} / {}",
        fmt_opt_f64(sample.game_x, 1),
        fmt_opt_f64(sample.game_z, 1)
    ));
    lines.push(String::new());

    lines.push("-- sectors (SectorsRecords) --".to_string());
    if let Some(src) = &sample.sectors_source {
        lines.push(format!("sectors_source: {src}"));
    }
    if let Some(dbg) = &sample.sectors_debug {
        lines.push(format!(
            "sectors_debug: array_num={} parsed={} first_err={}",
            dbg.array_num
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into()),
            dbg.parsed
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into()),
            dbg.first_err.as_deref().unwrap_or("-")
        ));
        if let Some(e) = &dbg.err {
            lines.push(format!("sectors_debug.err: {e}"));
        }
    }
    if sample.sectors.is_empty() {
        lines.push("sectors: (none yet)".to_string());
    } else {
        lines.push(format!("sectors[{}]:", sample.sectors.len()));
        for s in &sample.sectors {
            lines.push(format!(
                "  id={} cum_s={} split_s={}",
                s.id.map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into()),
                fmt_opt_f64(s.time_s, 3),
                fmt_opt_f64(s.split_s, 3),
            ));
        }
    }
    lines.push(String::new());

    if let Some(g) = &sample.ghost_ref {
        lines.push("-- ghost_ref --".to_string());
        if let Some(src) = &g.source {
            lines.push(format!("source: {src}"));
        }
        lines.push(format!(
            "ghost_diff_s: {}",
            fmt_opt_f64(g.diff_time_s, 3)
        ));
        lines.push(format!(
            "ghost_penalty_s: {}",
            fmt_opt_f64(g.penalty_total_s, 3)
        ));
        if g.sectors.is_empty() {
            lines.push("ghost_sectors: (none)".to_string());
        } else {
            lines.push(format!("ghost_sectors[{}]:", g.sectors.len()));
            for s in &g.sectors {
                lines.push(format!(
                    "  id={} cum_s={} split_s={}",
                    s.id.map(|v| v.to_string())
                        .unwrap_or_else(|| "?".into()),
                    fmt_opt_f64(s.time_s, 3),
                    fmt_opt_f64(s.split_s, 3),
                ));
            }
        }
        lines.push(String::new());
    }

    if opts.include_raw_json {
        lines.push("-- raw json --".to_string());
        if let Some(raw) = read_last_line(jsonl_path) {
            let max = 1200usize;
            if raw.len() > max {
                lines.push(format!("{}...", &raw[..max]));
            } else {
                lines.push(raw);
            }
        } else {
            lines.push("(empty)".to_string());
        }
    }

    lines.join("\n")
}

/// Backward-compatible wrapper (no raw JSON tail).
pub fn format_rtss_demo_text_simple(
    sample: &GameClockSample,
    file_age_sec: f64,
    jsonl_path: &Path,
) -> String {
    format_rtss_demo_text(sample, file_age_sec, jsonl_path, RtssDemoFormatOpts::default())
}

fn fmt_opt_f64(v: Option<f64>, prec: usize) -> String {
    match v {
        Some(x) if x.is_finite() => format!("{x:.prec$}"),
        Some(_) => "nan".into(),
        None => "-".into(),
    }
}

/// Truncate jsonl on session start (optional; keeps tail reads fast).
pub fn truncate_jsonl(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    f.write_all(b"{\"source\":\"acr_track_match\",\"event\":\"session_start\"}\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_light_sample_keeps_sectors() {
        let full = GameClockSample {
            race_time_s: Some(114.0),
            sectors: vec![GameClockSectorRecord {
                id: Some(0),
                time_s: Some(114.0),
                split_s: Some(114.0),
            }],
            sample_kind: Some("full".into()),
            ..GameClockSample::default_test()
        };
        let light = GameClockSample {
            race_time_s: Some(114.5),
            sectors: vec![],
            sample_kind: Some("light".into()),
            ..GameClockSample::default_test()
        };
        let merged = merge_game_clock_sample(Some(full), light);
        assert_eq!(merged.race_time_s, Some(114.5));
        assert_eq!(merged.sectors.len(), 1);
        assert_eq!(merged.sectors[0].split_s, Some(114.0));
    }

    #[test]
    fn parse_extended_sample() {
        let line = r#"{"race_time_s":20.25,"distance_m":353.1,"diff_time_s":-1.2,"position":1,"sectors":[{"id":0,"time_s":45.2,"split_s":12.1}],"ghost_ref":{"source":"ghost","diff_time_s":0.5,"sectors":[{"id":0,"time_s":44.7,"split_s":11.8}]}}"#;
        let s: GameClockSample = serde_json::from_str(line).unwrap();
        assert!((s.diff_time_s.unwrap() - (-1.2)).abs() < 1e-9);
        assert_eq!(s.sectors.len(), 1);
        assert_eq!(s.sectors[0].id, Some(0));
        assert!(s.ghost_ref.is_some());
    }

    #[test]
    fn rate_clamp_example() {
        let cfg = GameClockSyncConfig {
            enabled: true,
            jsonl_path: PathBuf::from("_test_no_jsonl.jsonl"),
            expected_tick_sec: 1.0,
            max_rate_adjust: 0.01,
            ..Default::default()
        };
        let mut c = GameClockCorrector::new(cfg);
        let sample = GameClockSample {
            race_time_s: Some(20.25),
            distance_m: Some(353.0),
            race_time_valid: true,
            diff_time_s: None,
            position: None,
            phase: None,
            sectors: vec![],
            sectors_source: None,
            sectors_debug: None,
            next_sector_index: None,
            race_source: None,
            travel_track_id: None,
            travel_track_source: None,
            penalty_total_s: None,
            ghost_ref: None,
            game_x: None,
            game_z: None,
            t_process_ms: None,
            t_wall_s: None,
            sample_kind: None,
        };
        c.apply_sample(sample.clone(), Instant::now(), Some(356.0));
        c.replica_time_sec = 20.20;
        c.apply_sample(sample, Instant::now(), Some(356.0));
        assert!((c.rate_time - 1.01).abs() < 0.0001);
        assert!((c.replica_time_sec - 20.2175).abs() < 0.001);
    }

    #[test]
    fn osd_pause_dash_while_frozen() {
        let cfg = GameClockSyncConfig {
            enabled: true,
            ..Default::default()
        };
        let mut c = GameClockCorrector::new(cfg);
        let mut pause = PauseOsdState::default();
        c.apply_sample(
            GameClockSample {
                race_time_s: Some(50.0),
                race_time_valid: false,
                ..GameClockSample::default_test()
            },
            Instant::now(),
            None,
        );
        assert!(c.osd_show_pause_dash(&mut pause));
    }

    #[test]
    fn pause_freezes_replica() {
        let cfg = GameClockSyncConfig {
            enabled: true,
            jsonl_path: PathBuf::from("_test_no_jsonl.jsonl"),
            ..Default::default()
        };
        let mut c = GameClockCorrector::new(cfg);
        let running = GameClockSample {
            race_time_s: Some(83.347),
            race_time_valid: true,
            ..GameClockSample::default_test()
        };
        c.apply_sample(running, Instant::now(), None);
        c.tick(1.0 / 333.0, None);
        let t_before = c.replica_race_time_sec().unwrap();
        let paused = GameClockSample {
            race_time_s: Some(83.347),
            race_time_valid: false,
            ..GameClockSample::default_test()
        };
        c.apply_sample(paused, Instant::now(), None);
        for _ in 0..500 {
            c.tick(1.0 / 333.0, None);
        }
        assert!((c.replica_race_time_sec().unwrap() - 83.347).abs() < 1e-6);
        assert!((t_before - 83.347).abs() < 0.01);
        assert_eq!(c.rate_time, 0.0);
    }
}

impl GameClockSample {
    fn default_test() -> Self {
        Self {
            race_time_s: None,
            distance_m: None,
            race_time_valid: false,
            diff_time_s: None,
            position: None,
            phase: None,
            sectors: vec![],
            sectors_source: None,
            sectors_debug: None,
            next_sector_index: None,
            race_source: None,
            travel_track_id: None,
            travel_track_source: None,
            penalty_total_s: None,
            ghost_ref: None,
            game_x: None,
            game_z: None,
            t_process_ms: None,
            t_wall_s: None,
            sample_kind: None,
        }
    }
}
