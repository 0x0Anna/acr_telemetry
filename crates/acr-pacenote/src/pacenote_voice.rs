//! Pre-recorded pacenote clips on a background playback queue.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use rodio::{Decoder, OutputStream, Sink, Source};
use serde::Deserialize;
use std::collections::BTreeMap;

fn default_trigger_radius_m() -> f64 {
    12.0
}
fn default_min_speed_kmh() -> f64 {
    8.0
}
fn default_volume() -> f32 {
    0.7
}
fn default_lead_sec() -> f64 {
    1.0
}
fn default_gear_advance_hz() -> u64 {
    5
}
fn default_gear_advance_gear() -> i32 {
    3
}
fn default_gear_reference_severity() -> u8 {
    6
}
fn default_gear_step_ms() -> u64 {
    300
}
fn default_skip_buffer_m() -> f64 {
    5.0
}
fn default_slow_corner_extra_lead_sec() -> f64 {
    1.0
}
fn default_protected_corner_gear() -> u8 {
    2
}
fn default_critical_start_reserve_sec() -> f64 {
    1.2
}
fn default_skippable_corner_gear() -> u8 {
    4
}
fn default_first_anchor_lock_radius_m() -> f64 {
    2.0
}
fn default_first_anchor_pick_max_speed_kmh() -> f64 {
    15.0
}
fn default_first_anchor_menu_radius_m() -> f64 {
    12.0
}

/// Voice clip basename for ambiguous pacenote start (`<voice_dir>/WhereDoWeWantToGo.wav`).
pub const PACENOTE_VOICE_WHERE_DO_WE_GO_TOKEN: &str = "WhereDoWeWantToGo";
/// Clip when pacenote stage context is lost (e.g. menu closed / left trigger zone).
pub const PACENOTE_VOICE_LOST_PACENOTES_TOKEN: &str = "LostPacenotes";
/// Clip after confirming an ambiguous pacenote stage pick.
pub const PACENOTE_VOICE_FOUND_PACENOTES_TOKEN: &str = "FoundPacenotes";

#[derive(Debug, Clone, Deserialize)]
pub struct PacenoteConfig {
    #[serde(default)]
    pub enabled: bool,
    pub pacenotes_dir: Option<PathBuf>,
    pub stage: Option<String>,
    pub geojson: Option<PathBuf>,
    pub voice_dir: Option<PathBuf>,
    #[serde(default = "default_trigger_radius_m")]
    pub trigger_radius_m: f64,
    #[serde(default = "default_min_speed_kmh")]
    pub min_speed_kmh: f64,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default = "default_lead_sec")]
    pub lead_sec: f64,
    #[serde(default = "default_gear_advance_hz")]
    pub gear_advance_hz: u64,
    #[serde(default = "default_gear_advance_gear")]
    pub gear_advance_gear: i32,
    #[serde(default = "default_gear_reference_severity")]
    pub gear_reference_severity: u8,
    #[serde(default = "default_gear_step_ms")]
    pub gear_step_ms: u64,
    #[serde(default = "default_skip_buffer_m")]
    pub skip_buffer_m: f64,
    #[serde(default = "default_slow_corner_extra_lead_sec")]
    pub slow_corner_extra_lead_sec: f64,
    #[serde(default = "default_protected_corner_gear")]
    pub protected_corner_gear: u8,
    #[serde(default = "default_critical_start_reserve_sec")]
    pub critical_start_reserve_sec: f64,
    #[serde(default = "default_skippable_corner_gear")]
    pub skippable_corner_gear: u8,
    #[serde(default = "default_first_anchor_lock_radius_m")]
    pub first_anchor_lock_radius_m: f64,
    #[serde(default = "default_first_anchor_pick_max_speed_kmh")]
    pub first_anchor_pick_max_speed_kmh: f64,
    /// Wider radius to detect multiple pacenote stages at the grid; `first_anchor_lock_radius_m` is still used for strict unique auto-lock.
    #[serde(default = "default_first_anchor_menu_radius_m")]
    pub first_anchor_menu_radius_m: f64,
    /// Reference SHP stem → ordered list of GeoJSON paths for that ref. Order is the default when
    /// several first anchors tie (same distance); first entry wins ambiguous same-spot cases.
    /// With player position + pacenote catalog, a unique anchor within `first_anchor_lock_radius_m`
    /// is auto-picked; see `pick_geojson_for_locked_reference`.
    #[serde(default)]
    pub ref_geojson_candidates: BTreeMap<String, Vec<std::path::PathBuf>>,
}

#[derive(Debug)]
enum PlayItem {
    Clip(PathBuf),
    Pause(Duration),
}

#[derive(Debug, Clone)]
struct PlaybackRequest {
    tokens: Vec<String>,
    urgency: u8,
}

pub struct PacenoteVoicePlayer {
    tx: mpsc::Sender<PlaybackRequest>,
}

impl PacenoteVoicePlayer {
    pub fn spawn(voice_dir: PathBuf, volume: f32) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || playback_worker(rx, voice_dir, volume));
        Self { tx }
    }

    pub fn enqueue(&self, tokens: Vec<String>, urgency: u8) {
        if tokens.is_empty() {
            return;
        }
        let _ = self.tx.send(PlaybackRequest { tokens, urgency });
    }
}

pub fn conflict_lead_advance_sec(
    voice_dir: &Path,
    tokens: &[String],
    callout_urgency: u8,
    next_urgency: Option<u8>,
    time_to_next_callout_sec: f64,
    cfg: &PacenoteConfig,
) -> f64 {
    if tokens.is_empty() {
        return 0.0;
    }
    let Some(next_urgency) = next_urgency else {
        return 0.0;
    };
    if next_urgency > cfg.protected_corner_gear || callout_urgency <= next_urgency {
        return 0.0;
    }
    let estimated_sec = estimate_tokens_duration_sec(voice_dir, tokens);
    let required_before_next = estimated_sec + cfg.critical_start_reserve_sec;
    (required_before_next - time_to_next_callout_sec).max(0.0)
}

fn playback_worker(rx: mpsc::Receiver<PlaybackRequest>, voice_dir: PathBuf, volume: f32) {
    let Ok((_stream, handle)) = OutputStream::try_default() else {
        eprintln!("pacenote voice: no audio output device");
        return;
    };
    let gain = volume.clamp(0.0, 2.0);
    while let Ok(mut request) = rx.recv() {
        while let Ok(next) = rx.try_recv() {
            if next.urgency < request.urgency {
                request = next;
            }
        }
        let Ok(sink) = Sink::try_new(&handle) else {
            continue;
        };
        for item in resolve_playback_items(&voice_dir, &request.tokens) {
            match item {
                PlayItem::Clip(path) => {
                    let Ok(file) = File::open(&path) else {
                        eprintln!("pacenote voice: missing {}", path.display());
                        continue;
                    };
                    let Ok(source) = Decoder::new(BufReader::new(file)) else {
                        eprintln!("pacenote voice: decode failed {}", path.display());
                        continue;
                    };
                    sink.append(source.amplify(gain));
                    sink.sleep_until_end();
                }
                PlayItem::Pause(duration) => {
                    if !duration.is_zero() {
                        thread::sleep(duration);
                    }
                }
            }
        }
        let _ = sink.stop();
    }
}

fn estimate_tokens_duration_sec(voice_dir: &Path, tokens: &[String]) -> f64 {
    resolve_playback_items(voice_dir, tokens)
        .iter()
        .map(|item| match item {
            PlayItem::Clip(path) => clip_duration_sec(path),
            PlayItem::Pause(duration) => duration.as_secs_f64(),
        })
        .sum()
}

fn clip_duration_sec(path: &Path) -> f64 {
    let Ok(file) = File::open(path) else {
        return 0.0;
    };
    let Ok(source) = Decoder::new(BufReader::new(file)) else {
        return 0.0;
    };
    source
        .total_duration()
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

fn resolve_playback_items(voice_dir: &Path, tokens: &[String]) -> Vec<PlayItem> {
    let mut remaining = tokens.to_vec();
    let mut out = Vec::new();
    while !remaining.is_empty() {
        if let Some(duration) = pause_duration(&remaining[0]) {
            out.push(PlayItem::Pause(duration));
            remaining.remove(0);
            continue;
        }
        let mut matched: Option<(usize, PathBuf)> = None;
        for len in (1..=remaining.len()).rev() {
            let key = remaining[..len].join("-");
            if let Some(path) = find_clip(voice_dir, &key) {
                matched = Some((len, path));
                break;
            }
        }
        if let Some((len, path)) = matched {
            out.push(PlayItem::Clip(path));
            remaining.drain(0..len);
        } else {
            eprintln!("pacenote voice: no clip for token {}", remaining[0]);
            remaining.remove(0);
        }
    }
    out
}

fn find_clip(voice_dir: &Path, key: &str) -> Option<PathBuf> {
    let direct = voice_dir.join(format!("{key}.wav"));
    if direct.is_file() {
        return Some(direct);
    }
    None
}

fn pause_duration(token: &str) -> Option<Duration> {
    let lower = token.to_ascii_lowercase();
    let rest = lower.strip_prefix("pause")?;
    let rest = rest.trim_end_matches("_reset");
    let seconds = rest.strip_suffix('s')?.parse::<f64>().ok()?;
    Some(Duration::from_secs_f64(seconds.max(0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_lead_advance_requests_extra_time() {
        let cfg = PacenoteConfig {
            enabled: true,
            pacenotes_dir: None,
            stage: None,
            geojson: None,
            voice_dir: None,
            trigger_radius_m: 12.0,
            min_speed_kmh: 8.0,
            volume: 0.7,
            lead_sec: 1.0,
            gear_advance_hz: 5,
            gear_advance_gear: 3,
            gear_reference_severity: 6,
            gear_step_ms: 300,
            skip_buffer_m: 5.0,
            slow_corner_extra_lead_sec: 1.0,
            protected_corner_gear: 2,
            critical_start_reserve_sec: 1.2,
            skippable_corner_gear: 4,
            first_anchor_lock_radius_m: 2.0,
            first_anchor_pick_max_speed_kmh: 15.0,
            first_anchor_menu_radius_m: 12.0,
            ref_geojson_candidates: BTreeMap::new(),
        };
        let advance = conflict_lead_advance_sec(
            Path::new("voices/elevenlabs_en"),
            &["Pause1.5s".into()],
            6,
            Some(1),
            1.0,
            &cfg,
        );
        assert!((advance - 1.7).abs() < 1e-6);
    }
}
