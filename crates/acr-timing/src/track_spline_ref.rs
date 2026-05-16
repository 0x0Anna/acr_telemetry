//! ACC `statics.track_spline_length` catalog for reference-track disambiguation.
//!
//! Calibrated values live in `timing/track_spline_lengths.toml`; [`default_catalog`]
//! embeds known entries so callers work without loading the file.

use std::collections::HashMap;
use std::path::Path;

/// Match tolerance after rounding to 0.1 m (float noise + one rounding step).
pub const SPLINE_LENGTH_MATCH_TOLERANCE_M: f32 = 0.15;

/// Round a spline length to the nearest 10 cm (game + reference values).
pub fn round_spline_length_m(m: f32) -> f32 {
    (m * 10.0).round() / 10.0
}

pub fn spline_lengths_match(observed: f32, reference_m: f32) -> bool {
    (round_spline_length_m(observed) - reference_m).abs() <= SPLINE_LENGTH_MATCH_TOLERANCE_M
}

/// Reference SHP stem → rounded `track_spline_length` (m).
pub fn default_catalog() -> &'static [(&'static str, f32)] {
    &[
        // Cwmbiga → Afon Biga (`reference_tracks/hafren_north.shp`); game 12077.95 m
        ("hafren_north", 12078.0),
    ]
}

pub fn default_catalog_map() -> HashMap<String, f32> {
    default_catalog()
        .iter()
        .map(|(k, v)| (k.to_string(), *v))
        .collect()
}

#[derive(Debug, serde::Deserialize)]
struct CatalogFile {
    #[serde(flatten)]
    tracks: HashMap<String, TrackSplineEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct TrackSplineEntry {
    length_m: f32,
}

/// Load `timing/track_spline_lengths.toml` (or any path). Missing file → embedded defaults.
pub fn load_catalog(path: &Path) -> Result<HashMap<String, f32>, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(default_catalog_map());
    }
    let raw = std::fs::read_to_string(path)?;
    let parsed: CatalogFile = toml::from_str(&raw)?;
    let mut out = HashMap::new();
    for (stem, entry) in parsed.tracks {
        out.insert(stem, round_spline_length_m(entry.length_m));
    }
    Ok(out)
}

/// Candidates among `ref_stems` whose catalog length matches `observed_m`.
pub fn matching_stems<'a>(
    observed_m: f32,
    ref_stems: impl IntoIterator<Item = &'a str>,
    catalog: &HashMap<String, f32>,
) -> Vec<&'a str> {
    ref_stems
        .into_iter()
        .filter(|stem| {
            catalog
                .get(*stem)
                .is_some_and(|ref_m| spline_lengths_match(observed_m, *ref_m))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hafren_north_game_value_matches() {
        assert!(spline_lengths_match(12077.95, 12078.0));
    }

    #[test]
    fn round_to_tenth_metre() {
        assert_eq!(round_spline_length_m(12077.95), 12078.0);
    }
}
