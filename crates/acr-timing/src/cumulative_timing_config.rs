//! GeoJSON-based cumulative subsection timing (replaces SHP ring for listed reference tracks).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::stage_timing_config::normalize_track_slug;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CumulativeTimingConfig {
    /// Directory with `{slug}.geojson` (same marker format as `timing/timing_sectors`).
    #[serde(default)]
    pub sectors_dir: Option<String>,
    #[serde(default)]
    pub gate_radius_m: Option<f64>,
    /// Reference track stem → GeoJSON file stem (without `.geojson`).
    /// Tracks listed here do **not** use `sectors_shp` subsection timing.
    #[serde(default)]
    pub ref_track_sectors: BTreeMap<String, String>,
}

impl CumulativeTimingConfig {
    pub fn sectors_dir(&self) -> PathBuf {
        PathBuf::from(
            self.sectors_dir
                .as_deref()
                .unwrap_or("timing/cumulative_sectors"),
        )
    }

    pub fn gate_radius_m(&self) -> f64 {
        self.gate_radius_m.unwrap_or(40.0)
    }

    pub fn slug_for_reference(&self, reference_track: &str) -> Option<&str> {
        let want = normalize_track_slug(reference_track);
        self.ref_track_sectors
            .iter()
            .find(|(k, _)| normalize_track_slug(k) == want)
            .map(|(_, slug)| slug.as_str())
    }

    pub fn uses_cumulative(&self, reference_track: &str) -> bool {
        self.slug_for_reference(reference_track).is_some()
    }
}
