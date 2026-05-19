//! `acr_timing.toml` — sector timing, grid, beeps, calibrated stage sectors.

use serde::Deserialize;

use crate::cumulative_timing_config::CumulativeTimingConfig;
use crate::split_beep::SplitBeepConfig;
use crate::stage_timing_config::StageTimingConfig;
use crate::timing_blame::BlameConfig;
use crate::timing_correlation::CorrelationConfig;
use crate::timing_frame_quality::TimingQualityConfig;
use crate::timing_voice::TimingVoiceConfig;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CorrelationConfigFile {
    /// Recompute timing_factors after each committed split (default: true).
    #[serde(default = "default_true")]
    pub auto_refresh: bool,
    pub slow_pct: Option<f64>,
    pub min_samples: Option<usize>,
}

fn default_true() -> bool {
    true
}

impl CorrelationConfigFile {
    pub fn to_runtime(&self) -> CorrelationConfig {
        CorrelationConfig {
            enabled: self.auto_refresh,
            slow_pct: self.slow_pct.unwrap_or(10.0),
            min_samples: self.min_samples.unwrap_or(4),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BlameConfigFile {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub min_delta_sec: Option<f64>,
    pub sigma_k: Option<f64>,
    pub max_factors: Option<usize>,
    pub min_samples: Option<usize>,
    pub slow_pct: Option<f64>,
}

impl BlameConfigFile {
    pub fn to_runtime(&self, correlation: &CorrelationConfig) -> BlameConfig {
        BlameConfig {
            enabled: self.enabled,
            min_delta_sec: self.min_delta_sec.unwrap_or(0.05),
            sigma_k: self.sigma_k.unwrap_or(2.0),
            max_factors: self.max_factors.unwrap_or(2),
            min_samples: self.min_samples.unwrap_or(correlation.min_samples),
            slow_pct: self.slow_pct.unwrap_or(correlation.slow_pct),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SubsectionHtmlConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Directory for `{track}_{car}_subsection_{timestamp}.html` (default: same as stage HTML dir).
    pub dir: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TimingConfigFile {
    pub sectors_shp: Option<String>,
    pub sectors_coord_space: Option<String>,
    pub sector_track_field: Option<String>,
    pub sector_id_field: Option<String>,
    pub timing_db: Option<String>,
    /// Human-readable personal bests for split deltas (`timing_pb.toml`).
    pub timing_pb: Option<String>,
    pub sector_cooldown_ms: Option<u64>,
    pub sector_radius: Option<f64>,
    pub start_points_geojson: Option<String>,
    pub start_prefilter_radius: Option<f64>,
    pub grid_standstill_max_speed_kmh: Option<f64>,
    pub grid_start_trigger_radius_m: Option<f64>,
    pub grid_start_list_radius_initial_m: Option<f64>,
    pub grid_start_wide_after_sec: Option<f64>,
    pub grid_start_list_radius_wide_m: Option<f64>,
    pub beep_on_split: Option<bool>,
    /// Cumulative GeoJSON gate crossings (uses `[cumulative_beep]`, not `[beep]`).
    pub beep_on_cumulative_split: Option<bool>,
    /// Legacy name for `beep_on_cumulative_split`.
    pub beep_on_silent_split: Option<bool>,
    #[serde(default)]
    pub beep: Option<SplitBeepConfig>,
    #[serde(default)]
    pub cumulative_beep: Option<SplitBeepConfig>,
    /// Legacy alias for `cumulative_beep`.
    #[serde(default)]
    pub silent_beep: Option<SplitBeepConfig>,
    #[serde(default)]
    pub subsection_html: SubsectionHtmlConfig,
    #[serde(default)]
    pub cumulative_timing: CumulativeTimingConfig,
    #[serde(default)]
    pub stage_timing: StageTimingConfig,
    #[serde(default)]
    pub correlation: CorrelationConfigFile,
    #[serde(default)]
    pub timing_blame: BlameConfigFile,
    #[serde(default)]
    pub timing_voice: TimingVoiceConfig,
    #[serde(default)]
    pub timing_quality: TimingQualityConfigFile,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TimingQualityConfigFile {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Subtract Σ max(0, Δt_wall − Δpacket/Hz) from stage leg times (default on).
    #[serde(default = "default_true")]
    pub apply_leg_excess_correction: bool,
    pub physics_hz: Option<f64>,
    pub max_wall_dt_ms: Option<f64>,
    pub pos_vel_slop_m: Option<f64>,
    pub pos_vel_rel_slop: Option<f64>,
    pub log_cooldown_ms: Option<f64>,
    /// Per-tick `[timing-suspect]` lines (default off).
    #[serde(default)]
    pub log_suspect_ticks: bool,
}

impl TimingQualityConfigFile {
    pub fn to_runtime(&self) -> TimingQualityConfig {
        TimingQualityConfig {
            enabled: self.enabled,
            physics_hz: self.physics_hz.unwrap_or(333.0),
            apply_leg_excess_correction: self.apply_leg_excess_correction,
            max_wall_dt_sec: self.max_wall_dt_ms.unwrap_or(80.0) / 1000.0,
            pos_vel_slop_m: self.pos_vel_slop_m.unwrap_or(1.5),
            pos_vel_rel_slop: self.pos_vel_rel_slop.unwrap_or(0.45),
            min_wall_dt_sec: 0.001,
            log_cooldown_sec: self.log_cooldown_ms.unwrap_or(250.0) / 1000.0,
            log_suspect_ticks: self.log_suspect_ticks,
        }
    }
}

pub const TIMING_CONFIG_FILE: &str = "acr_timing.toml";

pub fn config_search_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join(TIMING_CONFIG_FILE));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join(TIMING_CONFIG_FILE));
    }
    if let Some(config_dir) = dirs::config_dir() {
        paths.push(
            config_dir
                .join("acr_recorder")
                .join(TIMING_CONFIG_FILE),
        );
    }
    paths
}

pub fn load(path_override: Option<&std::path::Path>) -> Result<(TimingConfigFile, std::path::PathBuf), Box<dyn std::error::Error>> {
    if let Some(p) = path_override {
        if !p.exists() {
            return Err(format!("timing config not found: {}", p.display()).into());
        }
        let raw = std::fs::read_to_string(p)?;
        let cfg: TimingConfigFile = toml::from_str(&raw)?;
        return Ok((cfg, p.to_path_buf()));
    }
    for p in config_search_paths() {
        if p.exists() {
            let raw = std::fs::read_to_string(&p)?;
            let cfg: TimingConfigFile = toml::from_str(&raw)?;
            return Ok((cfg, p));
        }
    }
    Ok((TimingConfigFile::default(), std::path::PathBuf::from(TIMING_CONFIG_FILE)))
}

pub fn load_from_dir(dir: &std::path::Path) -> Result<TimingConfigFile, Box<dyn std::error::Error>> {
    let p = dir.join(TIMING_CONFIG_FILE);
    if !p.exists() {
        return Ok(TimingConfigFile::default());
    }
    let raw = std::fs::read_to_string(&p)?;
    Ok(toml::from_str(&raw)?)
}
