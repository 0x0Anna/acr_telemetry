//! Load `acr_track_match.toml`, `acr_timing.toml`, and `acr_pacenotes.toml`.

use std::path::{Path, PathBuf};

use acr_pacenote::pacenote_voice::PacenoteConfig;
use acr_timing::timing_config_file::TimingConfigFile;
use serde::Deserialize;

pub const TRACK_MATCH_CONFIG_FILE: &str = "acr_track_match.toml";
pub const PACENOTES_CONFIG_FILE: &str = "acr_pacenotes.toml";

#[derive(Debug, Deserialize, Default)]
pub struct TrackMatchConfigFile {
    pub refs: Option<Vec<String>>,
    pub input: Option<String>,
    pub live: Option<bool>,
    pub downsample: Option<usize>,
    pub buffer: Option<f64>,
    pub required_ratio: Option<f64>,
    pub history_points: Option<usize>,
    pub rate: Option<u64>,
    pub min_ref_spacing: Option<f64>,
    pub labels: Option<String>,
    pub rtss: Option<bool>,
    pub rtss_owner: Option<String>,
    pub rtss_slot: Option<u32>,
    pub rtss_clear_all: Option<bool>,
    pub track_keep_max_dist: Option<f64>,
    pub track_switch_min_gain: Option<f64>,
    pub track_lock_after_sec: Option<f64>,
    pub debug_physics_1hz: Option<bool>,
}

#[derive(Debug)]
pub struct LoadedAppConfig {
    pub track_match: TrackMatchConfigFile,
    pub timing: TimingConfigFile,
    pub pacenotes: Option<PacenoteConfig>,
    pub track_match_path: Option<PathBuf>,
    pub timing_path: Option<PathBuf>,
    pub pacenotes_path: Option<PathBuf>,
}

pub fn config_search_dirs(override_path: Option<&Path>) -> Vec<PathBuf> {
    if let Some(p) = override_path {
        let dir = p
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        return vec![dir.to_path_buf()];
    }
    let mut dirs = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    if let Some(config_dir) = dirs::config_dir() {
        dirs.push(config_dir.join("acr_recorder"));
    }
    dirs
}

pub fn load_all(
    track_match_override: Option<&Path>,
    timing_override: Option<&Path>,
    pacenotes_override: Option<&Path>,
) -> Result<LoadedAppConfig, Box<dyn std::error::Error>> {
    let dirs = config_search_dirs(track_match_override);

    let (track_match, track_match_path) =
        load_track_match_toml(track_match_override, &dirs)?;
    let (timing, timing_path) = load_timing_toml(timing_override, &dirs)?;
    let (pacenotes, pacenotes_path) = load_pacenotes_toml(pacenotes_override, &dirs)?;

    Ok(LoadedAppConfig {
        track_match,
        timing,
        pacenotes,
        track_match_path,
        timing_path,
        pacenotes_path,
    })
}

fn load_track_match_toml(
    override_path: Option<&Path>,
    dirs: &[PathBuf],
) -> Result<(TrackMatchConfigFile, Option<PathBuf>), Box<dyn std::error::Error>> {
    if let Some(p) = override_path {
        if !p.exists() {
            return Err(format!("config not found: {}", p.display()).into());
        }
        let raw = std::fs::read_to_string(p)?;
        return Ok((toml::from_str(&raw)?, Some(p.to_path_buf())));
    }
    for dir in dirs {
        let p = dir.join(TRACK_MATCH_CONFIG_FILE);
        if p.exists() {
            let raw = std::fs::read_to_string(&p)?;
            return Ok((toml::from_str(&raw)?, Some(p)));
        }
    }
    Ok((TrackMatchConfigFile::default(), None))
}

fn load_timing_toml(
    override_path: Option<&Path>,
    dirs: &[PathBuf],
) -> Result<(TimingConfigFile, Option<PathBuf>), Box<dyn std::error::Error>> {
    if let Some(p) = override_path {
        let (cfg, path) = acr_timing::timing_config_file::load(Some(p))?;
        return Ok((cfg, Some(path)));
    }
    for dir in dirs {
        let p = dir.join(acr_timing::timing_config_file::TIMING_CONFIG_FILE);
        if p.exists() {
            let raw = std::fs::read_to_string(&p)?;
            return Ok((toml::from_str(&raw)?, Some(p)));
        }
    }
    Ok((TimingConfigFile::default(), None))
}

fn load_pacenotes_toml(
    override_path: Option<&Path>,
    dirs: &[PathBuf],
) -> Result<(Option<PacenoteConfig>, Option<PathBuf>), Box<dyn std::error::Error>> {
    let path = if let Some(p) = override_path {
        if !p.exists() {
            return Err(format!("pacenotes config not found: {}", p.display()).into());
        }
        Some(p.to_path_buf())
    } else {
        dirs.iter()
            .map(|d| d.join(PACENOTES_CONFIG_FILE))
            .find(|p| p.exists())
    };
    let Some(p) = path else {
        return Ok((None, None));
    };
    let raw = std::fs::read_to_string(&p)?;
    let cfg: PacenoteConfig = toml::from_str(&raw)?;
    Ok((Some(cfg), Some(p)))
}
