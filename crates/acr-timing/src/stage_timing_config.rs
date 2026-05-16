//! Timing-only configuration (independent of pacenotes).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StageTimingConfig {
    /// HTML run logs: `{stage_slug}_{car}_{timestamp}.html`
    #[serde(default)]
    pub timing_sectors_html_dir: Option<String>,
    /// Per-slug sector GeoJSON directory (default `timing/timing_sectors`).
    #[serde(default)]
    pub sectors_dir: Option<String>,
    /// Reference track stem → calibrated stage slug(s) with `timing_sectors` GeoJSON.
    #[serde(default)]
    pub ref_stage_sectors: BTreeMap<String, StageSlugEntry>,
    /// Trigger radius for calibrated stage sector points (default 40 m).
    #[serde(default)]
    pub stage_sector_radius_m: Option<f64>,
    /// Log physics wheel position to stderr while speed ≤ `stillstand_max_speed_kmh` (default on).
    #[serde(default = "default_stillstand_position_log")]
    pub stillstand_position_log: bool,
    #[serde(default)]
    pub stillstand_max_speed_kmh: Option<f64>,
    #[serde(default)]
    pub stillstand_log_interval_sec: Option<f64>,
}

fn default_stillstand_position_log() -> bool {
    true
}

/// TOML value: `"cwmbiga_afon_biga"` or `["cwmbiga_afon_biga", "other"]`
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StageSlugEntry {
    One(String),
    Many(Vec<String>),
}

impl StageSlugEntry {
    pub fn primary(&self) -> Option<&str> {
        match self {
            StageSlugEntry::One(s) => Some(s.as_str()),
            StageSlugEntry::Many(v) => v.first().map(String::as_str),
        }
    }
}

impl StageTimingConfig {
    pub fn html_dir(&self) -> PathBuf {
        PathBuf::from(
            self.timing_sectors_html_dir
                .as_deref()
                .unwrap_or("timing/runs"),
        )
    }

    pub fn sectors_dir(&self) -> PathBuf {
        PathBuf::from(
            self.sectors_dir
                .as_deref()
                .unwrap_or("timing/timing_sectors"),
        )
    }

    pub fn stage_slug_for_reference(&self, reference_track: &str) -> Option<String> {
        let want = normalize_track_slug(reference_track);
        for (key, entry) in &self.ref_stage_sectors {
            if normalize_track_slug(key) == want {
                return entry.primary().map(str::to_string);
            }
        }
        None
    }

    pub fn path_for_stage_slug(&self, stage_slug: &str) -> PathBuf {
        self.sectors_dir().join(format!("{stage_slug}.geojson"))
    }

    pub fn stage_sector_radius_m(&self) -> f64 {
        self.stage_sector_radius_m.unwrap_or(40.0)
    }

    pub fn stillstand_position_log(&self) -> bool {
        self.stillstand_position_log
    }

    pub fn stillstand_max_speed_kmh(&self) -> f64 {
        self.stillstand_max_speed_kmh.unwrap_or(2.0)
    }

    pub fn stillstand_log_interval_sec(&self) -> f64 {
        self.stillstand_log_interval_sec.unwrap_or(2.0).max(0.5)
    }
}

pub fn normalize_track_slug(s: &str) -> String {
    let mut out = String::new();
    let mut prev_sep = false;
    for ch in s.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_sep = false;
        } else if !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_reference_track_slug() {
        let mut cfg = StageTimingConfig::default();
        cfg.ref_stage_sectors.insert(
            "hafren_north".to_string(),
            StageSlugEntry::One("cwmbiga_afon_biga".to_string()),
        );
        assert_eq!(
            cfg.stage_slug_for_reference("Hafren_North").as_deref(),
            Some("cwmbiga_afon_biga")
        );
    }
}
