//! `acr_timing.toml` — sector timing, grid, beeps, calibrated stage sectors.

use serde::Deserialize;

use crate::split_beep::SplitBeepConfig;
use crate::stage_timing_config::StageTimingConfig;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TimingConfigFile {
    pub sectors_shp: Option<String>,
    pub sectors_coord_space: Option<String>,
    pub sector_track_field: Option<String>,
    pub sector_id_field: Option<String>,
    pub timing_db: Option<String>,
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
    #[serde(default)]
    pub beep: Option<SplitBeepConfig>,
    #[serde(default)]
    pub stage_timing: StageTimingConfig,
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
