//! Map external timing provider `travel_track_id` → `reference_tracks` stem; used to lock track without spline correlation.

use std::collections::BTreeMap;

use crate::game_clock_sync::{read_latest_sample, GameClockSample};
use crate::stage_timing_config::StageTimingConfig;

fn normalize_key(s: &str) -> String {
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
    out.trim_matches('_').to_string()
}

/// Fresh JSONL sample (within `max_age_sec`).
pub fn fresh_game_clock_sample(
    jsonl_path: &std::path::Path,
    max_age_sec: f64,
) -> Option<GameClockSample> {
    read_latest_sample(jsonl_path, max_age_sec).map(|(s, _)| s)
}

pub fn game_clock_jsonl_live(jsonl_path: &std::path::Path, max_age_sec: f64) -> bool {
    fresh_game_clock_sample(jsonl_path, max_age_sec).is_some()
}

/// When true, do not use spline/start correlation to pick a reference track.
pub fn prefer_game_clock_track_lock(jsonl_path: &std::path::Path, max_age_sec: f64) -> bool {
    game_clock_jsonl_live(jsonl_path, max_age_sec)
}

/// Resolve `travel_track_id` to a `reference_tracks/*.shp` stem.
pub fn resolve_reference_track(
    travel_track_id: &str,
    reference_names: &[String],
    stage_timing: &StageTimingConfig,
    map: &BTreeMap<String, String>,
) -> Option<String> {
    let tid = travel_track_id.trim();
    if tid.is_empty() {
        return None;
    }
    for (k, v) in map {
        if k.eq_ignore_ascii_case(tid) {
            if reference_names.iter().any(|n| n == v) {
                return Some(v.clone());
            }
        }
    }
    let norm_tid = normalize_key(tid);
    if norm_tid.is_empty() {
        return None;
    }
    for name in reference_names {
        if normalize_key(name) == norm_tid {
            return Some(name.clone());
        }
    }
    for (ref_track, entry) in &stage_timing.ref_stage_sectors {
        for slug in entry.slugs() {
            if normalize_key(&slug) == norm_tid && reference_names.iter().any(|n| n == ref_track) {
                return Some(ref_track.clone());
            }
        }
        if normalize_key(ref_track) == norm_tid && reference_names.iter().any(|n| n == ref_track) {
            return Some(ref_track.clone());
        }
    }
    None
}

pub fn resolve_from_sample(
    sample: &GameClockSample,
    reference_names: &[String],
    stage_timing: &StageTimingConfig,
    map: &BTreeMap<String, String>,
) -> Option<String> {
    let tid = sample.travel_track_id.as_deref()?;
    resolve_reference_track(tid, reference_names, stage_timing, map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage_timing_config::StageSlugEntry;

    #[test]
    fn map_and_normalize() {
        let mut map = BTreeMap::new();
        map.insert("WalesS3HafrenNorth".into(), "hafren_north".into());
        let stage = StageTimingConfig {
            ref_stage_sectors: [(
                "hafren_north".into(),
                StageSlugEntry::One("cwmbiga_afon_biga".into()),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let refs = vec!["hafren_north".into(), "saverne".into()];
        assert_eq!(
            resolve_reference_track("WalesS3HafrenNorth", &refs, &stage, &map).as_deref(),
            Some("hafren_north")
        );
        assert_eq!(
            resolve_reference_track("cwmbiga_afon_biga", &refs, &stage, &map).as_deref(),
            Some("hafren_north")
        );
    }
}
