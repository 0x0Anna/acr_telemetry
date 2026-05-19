//! Split feedback on the default playback device (headphones / default route).
//! Configure in `acr_timing.toml` under `[beep]` and `[cumulative_beep]`.
//!
//! |Δ| tiers for **sine** beeps: ≤ 0.25 s → 1; ≤ 0.5 s → 2; else 3.
//! **WAV** plays once per split (tier picks which file); repetition belongs in the file.
//! `mode`: `sine` | `wav` | `both`. No queue — one thread per split.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use rodio::source::SineWave;
use rodio::{Decoder, OutputStream, Sink, Source};
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

fn play_wav(
    path: &Path,
    gain: f32,
    handle: &rodio::OutputStreamHandle,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let file = File::open(path)?;
    let sink = Sink::try_new(handle)?;
    let source = Decoder::new(BufReader::new(file))?.amplify(gain);
    sink.append(source);
    sink.sleep_until_end();
    Ok(())
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

fn play_split_feedback_blocking(delta: f64, cfg: &SplitBeepConfig) {
    let Ok((_stream, handle)) = OutputStream::try_default() else {
        eprintln!("split audio: no audio output device");
        return;
    };

    let beep_count = sine_beep_count(delta);
    let is_faster = delta <= 0.0;
    let gain = clamp_volume(cfg.volume);
    let mode = cfg.mode.to_lowercase();

    let want_wav = mode == "wav" || mode == "both";
    let mut played_wav = false;
    if want_wav {
        if let Some(path) = wav_path_for_tier(cfg, is_faster, beep_count) {
            if play_wav(path, gain, &handle).is_ok() {
                played_wav = true;
            }
        }
    }

    let sine_pulses = match mode.as_str() {
        "sine" => beep_count,
        "both" => beep_count,
        "wav" if !played_wav => beep_count,
        _ => 0,
    };

    for i in 0..sine_pulses {
        if let Err(e) = play_sine_pulse(is_faster, cfg, &handle) {
            eprintln!("split audio: {}", e);
        }
        if i + 1 < sine_pulses {
            thread::sleep(Duration::from_millis(cfg.gap_ms));
        }
    }
}

/// Split feedback; runs on a background thread (no queue).
pub fn play_split_feedback(delta: f64, cfg: &SplitBeepConfig) {
    let cfg = cfg.clone();
    thread::spawn(move || play_split_feedback_blocking(delta, &cfg));
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
}
