//! Split feedback on the default playback device (same routing as most apps — headphones OK).
//! Configure via `acr_track_match.toml` under `[beep]`.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
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
    /// `sine` = generated tones; `wav` = play `faster_wav` / `slower_wav` (falls back to sine if missing).
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
    /// Linear gain applied to sine and WAV (0 = silent, 1 = full sample/sine level, default 0.5).
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub faster_wav: Option<PathBuf>,
    #[serde(default)]
    pub slower_wav: Option<PathBuf>,
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
        }
    }
}

fn clamp_volume(v: f32) -> f32 {
    v.clamp(0.0, 2.0)
}

fn play_sine(
    hz: f32,
    duration_ms: u64,
    gain: f32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (_stream, handle) = OutputStream::try_default()?;
    let sink = Sink::try_new(&handle)?;
    let dur = Duration::from_millis(duration_ms.max(1));
    sink.append(SineWave::new(hz).take_duration(dur).amplify(gain));
    sink.sleep_until_end();
    Ok(())
}

fn play_wav(path: &PathBuf, gain: f32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let file = File::open(path)?;
    let (_stream, handle) = OutputStream::try_default()?;
    let sink = Sink::try_new(&handle)?;
    let source = Decoder::new(BufReader::new(file))?.amplify(gain);
    sink.append(source);
    sink.sleep_until_end();
    Ok(())
}

fn play_one(is_faster: bool, cfg: &SplitBeepConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let gain = clamp_volume(cfg.volume);
    let mode = cfg.mode.to_lowercase();
    if mode == "wav" {
        let path = if is_faster {
            cfg.faster_wav.as_ref()
        } else {
            cfg.slower_wav.as_ref()
        };
        if let Some(p) = path {
            if p.exists() {
                return play_wav(p, gain);
            }
        }
    }
    let (hz, ms) = if is_faster {
        (cfg.faster_freq_hz, cfg.faster_duration_ms)
    } else {
        (cfg.slower_freq_hz, cfg.slower_duration_ms)
    };
    play_sine(hz, ms, gain)
}

/// Tiered feedback by delta magnitude (same rules as before):
/// 0–250 ms: 1 beep; 250–500 ms: 2; >500 ms: 3.
pub fn play_split_feedback(delta: f64, cfg: &SplitBeepConfig) {
    let abs_ms = (delta.abs() * 1000.0) as i64;
    let count = if abs_ms <= 250 {
        1
    } else if abs_ms <= 500 {
        2
    } else {
        3
    };
    let is_faster = delta <= 0.0;
    for i in 0..count {
        if let Err(e) = play_one(is_faster, cfg) {
            eprintln!("split audio: {}", e);
        }
        if i + 1 < count {
            std::thread::sleep(Duration::from_millis(cfg.gap_ms));
        }
    }
}
