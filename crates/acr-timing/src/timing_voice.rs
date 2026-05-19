//! Pre-recorded WAV clips for slower-sector blame hints (`<voice_dir>/<Token>.wav`).

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use acc_shared_memory_rs::datatypes::Vector3f;
use rodio::{Decoder, OutputStream, Sink, Source};
use serde::Deserialize;

fn default_volume() -> f32 {
    0.75
}

fn default_true() -> bool {
    true
}

fn default_copilot_g_threshold() -> f32 {
    4.0
}

fn default_copilot_g_delay_sec() -> f64 {
    3.0
}

fn default_copilot_voice_cooldown_sec() -> f64 {
    45.0
}

fn default_copilot_crawl_max_speed_kmh() -> f32 {
    10.0
}

fn default_copilot_crawl_min_speed_kmh() -> f32 {
    1.0
}

/// WAV basename: `CopilotAreYouOkGoGoGo.wav` (full phrase).
pub const COPILOT_CRASH_VOICE_TOKEN: &str = "CopilotAreYouOkGoGoGo";

#[derive(Debug, Clone, Deserialize)]
pub struct TimingVoiceConfig {
    #[serde(default)]
    pub enabled: bool,
    pub voice_dir: Option<PathBuf>,
    #[serde(default = "default_volume")]
    pub volume: f32,
    /// After crash/reset or high-G: play copilot check-in clip.
    #[serde(default = "default_true")]
    pub copilot_crash_voice: bool,
    /// |g_force| magnitude threshold (ACC vector, typically ~1.0 = 1g).
    #[serde(default = "default_copilot_g_threshold")]
    pub copilot_g_threshold: f32,
    /// Seconds above threshold before the copilot clip plays.
    #[serde(default = "default_copilot_g_delay_sec")]
    pub copilot_g_delay_sec: f64,
    /// Minimum time between copilot crash clips.
    #[serde(default = "default_copilot_voice_cooldown_sec")]
    pub copilot_voice_cooldown_sec: f64,
    /// After high-G (see delay): play when speed is in [min, max) km/h (crawl after impact).
    #[serde(default = "default_copilot_crawl_min_speed_kmh")]
    pub copilot_crawl_min_speed_kmh: f32,
    #[serde(default = "default_copilot_crawl_max_speed_kmh")]
    pub copilot_crawl_max_speed_kmh: f32,
}

impl Default for TimingVoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            voice_dir: None,
            volume: default_volume(),
            copilot_crash_voice: true,
            copilot_g_threshold: default_copilot_g_threshold(),
            copilot_g_delay_sec: default_copilot_g_delay_sec(),
            copilot_voice_cooldown_sec: default_copilot_voice_cooldown_sec(),
            copilot_crawl_min_speed_kmh: default_copilot_crawl_min_speed_kmh(),
            copilot_crawl_max_speed_kmh: default_copilot_crawl_max_speed_kmh(),
        }
    }
}

/// Schedules copilot voice after heavy impacts (high G → crawl speed window).
#[derive(Debug, Clone, Default)]
pub struct CopilotCrashVoiceState {
    g_over_threshold_since: Option<Instant>,
    last_played: Option<Instant>,
    /// Armed after sustained high G; clip plays on first crawl in speed window.
    pending_after_high_g: bool,
}

impl CopilotCrashVoiceState {
    pub fn clear_g_pending(&mut self) {
        self.g_over_threshold_since = None;
    }

    pub fn clear_copilot_pending(&mut self) {
        self.pending_after_high_g = false;
        self.g_over_threshold_since = None;
    }

    fn may_arm_copilot(&self, cfg: &TimingVoiceConfig) -> bool {
        if self.pending_after_high_g {
            return false;
        }
        if let Some(last) = self.last_played {
            if last.elapsed().as_secs_f64() < cfg.copilot_voice_cooldown_sec {
                return false;
            }
        }
        true
    }

    pub fn g_force_magnitude(g: &Vector3f) -> f64 {
        let x = g.x as f64;
        let y = g.y as f64;
        let z = g.z as f64;
        (x * x + y * y + z * z).sqrt()
    }

    pub fn observe_high_g(&mut self, g_force: &Vector3f, cfg: &TimingVoiceConfig) {
        if !cfg.enabled || !cfg.copilot_crash_voice {
            return;
        }
        let now = Instant::now();
        let g_mag = Self::g_force_magnitude(g_force);
        let thr = cfg.copilot_g_threshold as f64;
        if g_mag >= thr {
            if self.g_over_threshold_since.is_none() {
                self.g_over_threshold_since = Some(now);
            }
            let since = self.g_over_threshold_since.unwrap();
            if now.duration_since(since).as_secs_f64() >= cfg.copilot_g_delay_sec
                && self.may_arm_copilot(cfg)
            {
                self.pending_after_high_g = true;
                self.g_over_threshold_since = None;
                eprintln!(
                    "timing voice: copilot armed after high G (plays when {:.0}–{:.0} km/h crawl)",
                    cfg.copilot_crawl_min_speed_kmh,
                    cfg.copilot_crawl_max_speed_kmh
                );
            }
        } else {
            self.g_over_threshold_since = None;
        }
    }

    pub fn observe_speed_for_pending_copilot(
        &mut self,
        speed_kmh: f64,
        voice: Option<&TimingVoicePlayer>,
        cfg: &TimingVoiceConfig,
    ) {
        if !self.pending_after_high_g || !cfg.enabled || !cfg.copilot_crash_voice {
            return;
        }
        let min = cfg.copilot_crawl_min_speed_kmh as f64;
        let max = cfg.copilot_crawl_max_speed_kmh as f64;
        if speed_kmh >= max {
            return;
        }
        if speed_kmh >= min {
            self.pending_after_high_g = false;
            self.try_play(
                voice,
                cfg,
                &format!("slow crawl after high G ({speed_kmh:.1} km/h)"),
            );
        }
    }

    fn try_play(
        &mut self,
        voice: Option<&TimingVoicePlayer>,
        cfg: &TimingVoiceConfig,
        reason: &str,
    ) {
        let now = Instant::now();
        if let Some(last) = self.last_played {
            if now.duration_since(last).as_secs_f64() < cfg.copilot_voice_cooldown_sec {
                return;
            }
        }
        let Some(voice) = voice else {
            return;
        };
        eprintln!(
            "timing voice: copilot crash check ({reason}) → {}",
            COPILOT_CRASH_VOICE_TOKEN
        );
        voice.enqueue(vec![COPILOT_CRASH_VOICE_TOKEN.to_string()]);
        self.last_played = Some(now);
        self.g_over_threshold_since = None;
    }
}

#[derive(Debug, Clone)]
struct PlaybackRequest {
    tokens: Vec<String>,
}

pub struct TimingVoicePlayer {
    tx: mpsc::Sender<PlaybackRequest>,
}

impl TimingVoicePlayer {
    pub fn spawn(voice_dir: PathBuf, volume: f32) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || playback_worker(rx, voice_dir, volume));
        Self { tx }
    }

    pub fn enqueue(&self, tokens: Vec<String>) {
        if tokens.is_empty() {
            return;
        }
        let _ = self.tx.send(PlaybackRequest { tokens });
    }
}

fn playback_worker(rx: mpsc::Receiver<PlaybackRequest>, voice_dir: PathBuf, volume: f32) {
    let Ok((_stream, handle)) = OutputStream::try_default() else {
        eprintln!("timing voice: no audio output device");
        return;
    };
    let gain = volume.clamp(0.0, 2.0);
    while let Ok(request) = rx.recv() {
        let sink = Sink::try_new(&handle).ok();
        let Some(sink) = sink else { continue };
        for token in &request.tokens {
            let Some(path) = find_clip(&voice_dir, token) else {
                eprintln!("timing voice: missing clip for token {token}");
                continue;
            };
            let Ok(file) = File::open(&path) else {
                eprintln!("timing voice: cannot open {}", path.display());
                continue;
            };
            let Ok(source) = Decoder::new(BufReader::new(file)) else {
                eprintln!("timing voice: decode failed {}", path.display());
                continue;
            };
            sink.append(source.amplify(gain));
        }
        sink.sleep_until_end();
    }
}

fn find_clip(voice_dir: &Path, token: &str) -> Option<PathBuf> {
    let direct = voice_dir.join(format!("{token}.wav"));
    if direct.is_file() {
        return Some(direct);
    }
    None
}
