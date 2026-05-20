//! Split feedback on the default playback device (headphones / default route).
//! Configure in `acr_timing.toml` under `[beep]` and `[cumulative_beep]`.
//!
//! |Δ| tiers for **sine** beeps: ≤ 0.25 s → 1; ≤ 0.5 s → 2; else 3.
//! **WAV** plays once per split (tier picks which file); repetition belongs in the file.
//! `mode`: `sine` | `wav` | `both`. No queue — one thread per split.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use hound::{SampleFormat, WavReader};
use rodio::buffer::SamplesBuffer;
use rodio::source::SineWave;
use rodio::{OutputStream, Sink, Source};
use serde::Deserialize;

fn default_mode() -> String {
    "sine".into()
}
fn default_faster_freq() -> f32 {
    1700.0
}
fn default_faster_ms() -> u64 {
    45
}
fn default_slower_freq() -> f32 {
    950.0
}
fn default_slower_ms() -> u64 {
    140
}
fn default_gap_ms() -> u64 {
    200
}
fn default_volume() -> f32 {
    0.5
}

#[derive(Debug, Clone, Deserialize)]
pub struct SplitBeepConfig {
    /// `sine` = tones only; `wav` = one WAV per split (sine fallback if missing);
    /// `both` = one WAV then 1–3 sine beeps.
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_faster_freq")]
    pub faster_freq_hz: f32,
    #[serde(default = "default_faster_ms")]
    pub faster_duration_ms: u64,
    #[serde(default = "default_slower_freq")]
    pub slower_freq_hz: f32,
    #[serde(default = "default_slower_ms")]
    pub slower_duration_ms: u64,
    #[serde(default = "default_gap_ms")]
    pub gap_ms: u64,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub faster_wav: Option<PathBuf>,
    #[serde(default)]
    pub slower_wav: Option<PathBuf>,
    /// Optional WAV per |Δ| tier (1 / 2 / 3 sine beeps would have fired); played once only.
    #[serde(default)]
    pub faster_wav_1: Option<PathBuf>,
    #[serde(default)]
    pub faster_wav_2: Option<PathBuf>,
    #[serde(default)]
    pub faster_wav_3: Option<PathBuf>,
    #[serde(default)]
    pub slower_wav_1: Option<PathBuf>,
    #[serde(default)]
    pub slower_wav_2: Option<PathBuf>,
    #[serde(default)]
    pub slower_wav_3: Option<PathBuf>,
}

impl Default for SplitBeepConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            faster_freq_hz: default_faster_freq(),
            faster_duration_ms: default_faster_ms(),
            slower_freq_hz: default_slower_freq(),
            slower_duration_ms: default_slower_ms(),
            gap_ms: default_gap_ms(),
            volume: default_volume(),
            faster_wav: None,
            slower_wav: None,
            faster_wav_1: None,
            faster_wav_2: None,
            faster_wav_3: None,
            slower_wav_1: None,
            slower_wav_2: None,
            slower_wav_3: None,
        }
    }
}

fn clamp_volume(v: f32) -> f32 {
    v.clamp(0.0, 2.0)
}

/// `beep_count` = 1, 2, or 3 (magnitude tier) → which tier WAV to use.
pub fn wav_path_for_tier(cfg: &SplitBeepConfig, is_faster: bool, beep_count: u32) -> Option<&Path> {
    let tier = beep_count.saturating_sub(1).min(2) as usize;
    let tier_path = if is_faster {
        match tier {
            0 => cfg.faster_wav_1.as_deref(),
            1 => cfg.faster_wav_2.as_deref(),
            _ => cfg.faster_wav_3.as_deref(),
        }
    } else {
        match tier {
            0 => cfg.slower_wav_1.as_deref(),
            1 => cfg.slower_wav_2.as_deref(),
            _ => cfg.slower_wav_3.as_deref(),
        }
    };
    tier_path
        .or_else(|| {
            if is_faster {
                cfg.faster_wav.as_deref()
            } else {
                cfg.slower_wav.as_deref()
            }
        })
        .filter(|p| p.is_file())
}

fn sine_beep_count(delta: f64) -> u32 {
    let abs_ms = (delta.abs() * 1000.0) as i64;
    if abs_ms <= 250 {
        1
    } else if abs_ms <= 500 {
        2
    } else {
        3
    }
}

fn play_sine(
    hz: f32,
    duration_ms: u64,
    gain: f32,
    handle: &rodio::OutputStreamHandle,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sink = Sink::try_new(handle)?;
    let dur = Duration::from_millis(duration_ms.max(1));
    sink.append(SineWave::new(hz).take_duration(dur).amplify(gain));
    sink.sleep_until_end();
    Ok(())
}

/// Decode via `hound` (24-bit PCM etc.); rodio's built-in WAV decoder often plays those silently.
fn play_wav(
    path: &Path,
    gain: f32,
    handle: &rodio::OutputStreamHandle,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut reader = WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels.max(1);
    let bits = spec.bits_per_sample;
    let max_i = (1i64 << (bits.saturating_sub(1))) as f32;

    let samples: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.unwrap_or(0.0))
            .collect(),
        SampleFormat::Int if bits <= 8 => reader
            .samples::<i8>()
            .map(|s| s.unwrap_or(0) as f32 / i8::MAX as f32)
            .collect(),
        SampleFormat::Int if bits <= 16 => reader
            .samples::<i16>()
            .map(|s| s.unwrap_or(0) as f32 / i16::MAX as f32)
            .collect(),
        SampleFormat::Int => reader
            .samples::<i32>()
            .map(|s| s.unwrap_or(0) as f32 / max_i)
            .collect(),
    };
    if samples.is_empty() {
        return Err("empty WAV".into());
    }
    let sink = Sink::try_new(handle)?;
    let source = SamplesBuffer::new(channels, spec.sample_rate, samples).amplify(gain);
    sink.append(source);
    sink.sleep_until_end();
    Ok(())
}

/// Log resolved WAV paths once at startup (stderr).
pub fn log_wav_paths(label: &str, cfg: &SplitBeepConfig) {
    eprintln!(
        "split audio [{label}]: mode={} volume={:.2}",
        cfg.mode, cfg.volume
    );
    for (name, path) in [
        ("faster_wav_1", cfg.faster_wav_1.as_deref()),
        ("slower_wav_1", cfg.slower_wav_1.as_deref()),
        ("faster_wav", cfg.faster_wav.as_deref()),
        ("slower_wav", cfg.slower_wav.as_deref()),
    ] {
        if let Some(p) = path {
            let ok = p.is_file();
            eprintln!(
                "split audio [{label}]: {name}={} exists={ok}",
                p.display()
            );
        }
    }
}

fn play_sine_pulse(
    is_faster: bool,
    cfg: &SplitBeepConfig,
    handle: &rodio::OutputStreamHandle,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let gain = clamp_volume(cfg.volume);
    let (hz, ms) = if is_faster {
        (cfg.faster_freq_hz, cfg.faster_duration_ms)
    } else {
        (cfg.slower_freq_hz, cfg.slower_duration_ms)
    };
    play_sine(hz, ms, gain, handle)
}

struct BeepJob {
    delta: f64,
    cfg: SplitBeepConfig,
}

struct SplitBeepPlayer {
    tx: mpsc::Sender<BeepJob>,
}

static BEEP_PLAYER: OnceLock<SplitBeepPlayer> = OnceLock::new();

fn beep_player() -> &'static SplitBeepPlayer {
    BEEP_PLAYER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || split_beep_worker(rx));
        SplitBeepPlayer { tx }
    })
}

fn split_beep_worker(rx: mpsc::Receiver<BeepJob>) {
    let Ok((_stream, handle)) = OutputStream::try_default() else {
        eprintln!("split audio: no audio output device");
        return;
    };
    while let Ok(job) = rx.recv() {
        play_split_feedback_on_handle(job.delta, &job.cfg, &handle);
    }
}

fn play_split_feedback_on_handle(
    delta: f64,
    cfg: &SplitBeepConfig,
    handle: &rodio::OutputStreamHandle,
) {
    if !delta.is_finite() {
        return;
    }

    let beep_count = sine_beep_count(delta);
    let is_faster = delta <= 0.0;
    let gain = clamp_volume(cfg.volume);
    let mode = cfg.mode.to_lowercase();

    let want_wav = mode == "wav" || mode == "both";
    let mut played_wav = false;
    if want_wav {
        if let Some(path) = wav_path_for_tier(cfg, is_faster, beep_count) {
            match play_wav(path, gain, handle) {
                Ok(()) => played_wav = true,
                Err(e) => eprintln!(
                    "split audio: WAV {} (Δ={delta:+.3}s faster={is_faster}) — {e}",
                    path.display()
                ),
            }
        } else {
            eprintln!(
                "split audio: no WAV (Δ={delta:+.3}s faster={is_faster} tier={beep_count})"
            );
        }
    }

    let sine_pulses = match mode.as_str() {
        "sine" => beep_count,
        "both" => beep_count,
        "wav" if !played_wav => beep_count,
        _ => 0,
    };

    for i in 0..sine_pulses {
        if let Err(e) = play_sine_pulse(is_faster, cfg, handle) {
            eprintln!("split audio: {}", e);
        }
        if i + 1 < sine_pulses {
            thread::sleep(Duration::from_millis(cfg.gap_ms));
        }
    }
}

/// Queue split feedback on a single output stream (avoids cut-off from per-beep streams).
pub fn play_split_feedback(delta: f64, cfg: &SplitBeepConfig) {
    if !delta.is_finite() {
        return;
    }
    let _ = beep_player().tx.send(BeepJob {
        delta,
        cfg: cfg.clone(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_wav_maps_beep_count() {
        let cfg = SplitBeepConfig {
            faster_wav_2: Some(PathBuf::from("/nonexistent_tier2.wav")),
            faster_wav: Some(PathBuf::from("/nonexistent_default.wav")),
            ..Default::default()
        };
        assert!(wav_path_for_tier(&cfg, true, 2).is_none());
        assert!(wav_path_for_tier(&cfg, true, 1).is_none());
    }

    #[test]
    fn sine_beep_count_tiers() {
        assert_eq!(sine_beep_count(0.1), 1);
        assert_eq!(sine_beep_count(0.4), 2);
        assert_eq!(sine_beep_count(0.9), 3);
    }

    #[test]
    fn good_wav_has_samples_via_hound() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/split_sounds/good.wav");
        assert!(path.is_file(), "missing {}", path.display());
        let mut reader = hound::WavReader::open(&path).unwrap();
        assert!(reader.samples::<i32>().count() > 1000);
    }

    #[test]
    fn bad_wav_has_samples_via_hound() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/split_sounds/bad.wav");
        assert!(path.is_file(), "missing {}", path.display());
        let mut reader = hound::WavReader::open(&path).unwrap();
        assert!(reader.samples::<i32>().count() > 1000);
    }

    #[test]
    fn slower_tier_picks_bad_not_good() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/split_sounds");
        let cfg = SplitBeepConfig {
            faster_wav_1: Some(base.join("good.wav")),
            slower_wav_1: Some(base.join("bad.wav")),
            ..Default::default()
        };
        assert!(wav_path_for_tier(&cfg, false, 1).unwrap().ends_with("bad.wav"));
        assert!(wav_path_for_tier(&cfg, true, 1).unwrap().ends_with("good.wav"));
    }
}
