//! Path resolution shared by timing and recorder tools.

use std::path::{Path, PathBuf};

fn base_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// Resolve a path (relative or absolute). Relative paths use the executable directory (fallback: CWD).
pub fn resolve_path(s: &str) -> PathBuf {
    let p = Path::new(s);
    if p.is_absolute() {
        p.to_path_buf()
    } else if let Some(base) = base_dir().or_else(|| std::env::current_dir().ok()) {
        base.join(p)
    } else {
        p.to_path_buf()
    }
}

/// Default notes / timing DB directory (`%APPDATA%/acr_telemetry` on Windows).
pub fn default_notes_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("acr_telemetry"))
        .unwrap_or_else(|| PathBuf::from("."))
}
