//! Single config file for settings that belong to the launcher itself,
//! not the underlying CLI tools it drives.
//!
//! `acr_recorder.toml` (see `acr_recorder::config`) is a different,
//! *shared* file — it's read by `acr_recorder`/`acr_export`/`acr_motec`/
//! `acr_track_match` whether or not the launcher is involved, and the
//! Record/Export tabs edit it because that's genuinely where those
//! settings live for the standalone tools. This file is the opposite:
//! nothing in it means anything to a CLI tool run without the launcher,
//! so it gets its own file rather than polluting the shared one.
//!
//! Holds `[hotkeys]` (see `hotkeys.rs`), `[export]` (checkbox state the
//! Export tab remembers — the SQLite/output-dir *paths* it also edits live
//! in the shared `acr_recorder.toml` instead, alongside the CLI tools' own
//! config, same as the Record tab), `[track_match]` (the last-picked
//! reference track file(s)/folder — kept here rather than
//! `acr_track_match.toml` since the launcher always passes `--refs`
//! explicitly, so this is purely "remember what I picked last time", not
//! something the standalone CLI tool needs), `[plot_recording]`/
//! `[grip_estimator]` (last-used input paths for those two purely
//! CLI-flag-driven, config-file-less tools), and `[telemetry_bridge]`
//! (last-used rate/UDP/HTTP/unit settings — the panel writes a fresh
//! `acr_telemetry_bridge.toml` from these on every Start rather than
//! reading them back from that file). Any future launcher-only preference
//! (window size, theme, …) belongs here as a new field/table on
//! `LauncherConfig`, not a new file.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::hotkeys::HotkeyFileConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LauncherConfig {
    #[serde(default)]
    pub(crate) hotkeys: HotkeyFileConfig,
    #[serde(default)]
    pub(crate) export: ExportUiConfig,
    #[serde(default)]
    pub(crate) track_match: TrackMatchUiConfig,
    #[serde(default)]
    pub(crate) plot_recording: PlotRecordingUiConfig,
    #[serde(default)]
    pub(crate) grip_estimator: GripEstimatorUiConfig,
    #[serde(default)]
    pub(crate) telemetry_bridge: TelemetryBridgeUiConfig,
    #[serde(default)]
    pub(crate) analysis_export: AnalysisExportUiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExportUiConfig {
    #[serde(default = "default_true")]
    pub(crate) do_csv: bool,
    #[serde(default)]
    pub(crate) do_sqlite: bool,
}

impl Default for ExportUiConfig {
    fn default() -> Self {
        Self {
            do_csv: true,
            do_sqlite: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct TrackMatchUiConfig {
    #[serde(default)]
    pub(crate) refs: Vec<String>,
}

/// Last-used input file's directory (Plot Recording tab), so the file
/// picker re-opens where the user left off — the tool itself takes no
/// config, so nothing else about this tab needs persisting.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PlotRecordingUiConfig {
    #[serde(default)]
    pub(crate) last_dir: Option<String>,
}

/// Last-used input path/mode (Grip Estimator tab) — the tool itself takes
/// no config file, purely CLI flags, so this is the only persistence for it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GripEstimatorUiConfig {
    #[serde(default)]
    pub(crate) use_sqlite_mode: bool,
    #[serde(default)]
    pub(crate) last_sqlite_path: Option<String>,
    #[serde(default)]
    pub(crate) last_rkyv_path: Option<String>,
}

/// Last-used rate/UDP/HTTP/unit settings (Telemetry Bridge tab). These
/// mirror `acr_telemetry_bridge`'s own `BridgeConfig` fields but live here
/// rather than in `acr_telemetry_bridge.toml` directly, since the panel
/// writes that TOML fresh from the UI on every Start — this is just what
/// pre-fills the UI itself next launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TelemetryBridgeUiConfig {
    #[serde(default = "default_bridge_rate_hz")]
    pub(crate) rate_hz: u64,
    #[serde(default)]
    pub(crate) udp_enabled: bool,
    #[serde(default)]
    pub(crate) udp_target: String,
    #[serde(default)]
    pub(crate) http_enabled: bool,
    #[serde(default = "default_bridge_http_addr")]
    pub(crate) http_addr: String,
    #[serde(default = "default_bridge_unit")]
    pub(crate) temperature_unit: String,
}

impl Default for TelemetryBridgeUiConfig {
    fn default() -> Self {
        Self {
            rate_hz: default_bridge_rate_hz(),
            udp_enabled: false,
            udp_target: String::new(),
            http_enabled: true,
            http_addr: default_bridge_http_addr(),
            temperature_unit: default_bridge_unit(),
        }
    }
}

fn default_bridge_rate_hz() -> u64 {
    5
}

fn default_bridge_http_addr() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_bridge_unit() -> String {
    "c".to_string()
}

/// Last-used recording ID + path overrides (Analysis Export tab) — the
/// tool itself takes no config file of its own (all flags, see
/// `src/bin/acr_analysis_export.rs`), so this is the only persistence for
/// it, same shape as `GripEstimatorUiConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AnalysisExportUiConfig {
    #[serde(default)]
    pub(crate) last_recording_id: Option<String>,
    #[serde(default)]
    pub(crate) last_grafana_db: Option<String>,
    #[serde(default)]
    pub(crate) last_telemetry_db: Option<String>,
    #[serde(default)]
    pub(crate) last_analysis_db: Option<String>,
    /// `--serve` mode's last-used port — matches the tool's own default
    /// (`9876`, see `src/bin/acr_analysis_export.rs`) and the port baked
    /// into `grafana/AC Rally full-dashboard.json`'s "Export Annotation
    /// ranges to analysis" link, so the two stay in sync unless changed.
    #[serde(default = "default_serve_port")]
    pub(crate) last_serve_port: u16,
}

impl Default for AnalysisExportUiConfig {
    fn default() -> Self {
        Self {
            last_recording_id: None,
            last_grafana_db: None,
            last_telemetry_db: None,
            last_analysis_db: None,
            last_serve_port: default_serve_port(),
        }
    }
}

fn default_serve_port() -> u16 {
    9876
}

/// Next to the launcher's own executable, same convention
/// `acr_recorder::config::base_dir()`/`recorder_panel.rs` use for
/// `acr_recorder.toml`.
fn config_file_path() -> PathBuf {
    acr_recorder::config::base_dir()
        .map(|dir| dir.join("acr_launcher.toml"))
        .unwrap_or_else(|| PathBuf::from("acr_launcher.toml"))
}

pub(crate) fn load() -> LauncherConfig {
    let path = config_file_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        match toml::from_str(&text) {
            Ok(cfg) => return cfg,
            Err(e) => crate::process::log_err(format!("launcher_config: failed to parse {}: {e}", path.display())),
        }
    }
    LauncherConfig::default()
}

pub(crate) fn save(cfg: &LauncherConfig) {
    let path = config_file_path();
    match toml::to_string_pretty(cfg) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                crate::process::log_err(format!("launcher_config: failed to write {}: {e}", path.display()));
            }
        }
        Err(e) => crate::process::log_err(format!("launcher_config: failed to serialize: {e}")),
    }
}
