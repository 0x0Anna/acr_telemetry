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
//! Currently holds just `[hotkeys]` (see `hotkeys.rs`), but any future
//! launcher-only preference (window size, theme, …) belongs here as a
//! new field/table on `LauncherConfig`, not a new file.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::hotkeys::HotkeyFileConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LauncherConfig {
    #[serde(default)]
    pub(crate) hotkeys: HotkeyFileConfig,
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
            Err(e) => eprintln!("launcher_config: failed to parse {}: {e}", path.display()),
        }
    }
    LauncherConfig::default()
}

pub(crate) fn save(cfg: &LauncherConfig) {
    let path = config_file_path();
    match toml::to_string_pretty(cfg) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                eprintln!("launcher_config: failed to write {}: {e}", path.display());
            }
        }
        Err(e) => eprintln!("launcher_config: failed to serialize: {e}"),
    }
}
