//! Pacenote callouts loaded from converted GeoJSON.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::gis;

#[derive(Debug, Clone)]
pub struct PacenoteStageSummary {
    pub slug: String,
    pub path: PathBuf,
    pub reference_track: String,
    pub stage: String,
    pub start_x: f64,
    pub start_z: f64,
    pub max_distance_m: f64,
}

#[derive(Debug, Clone)]
pub struct PacenoteStagePick {
    pub reference_track: String,
    pub slug: String,
    pub path: PathBuf,
    pub stage: String,
}

#[derive(Debug, Default)]
pub struct PacenoteStageCatalog {
    stages: Vec<PacenoteStageSummary>,
}

#[derive(Debug, Clone)]
pub struct PacenoteCallout {
    pub index: usize,
    pub x: f64,
    pub z: f64,
    pub notes: Vec<String>,
    pub notes_text: String,
    pub link_to_next: bool,
    pub max_turn_severity: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct PacenoteCourse {
    pub stage: String,
    pub reference_track: String,
    pub callouts: Vec<PacenoteCallout>,
    pub leg_distance_m: Vec<f64>,
}

#[derive(Debug, Deserialize)]
struct GeoJsonRoot {
    #[serde(default)]
    properties: GeoJsonCollectionProperties,
    #[serde(default)]
    features: Vec<GeoJsonFeature>,
}

#[derive(Debug, Default, Deserialize)]
struct GeoJsonCollectionProperties {
    #[serde(default)]
    max_pacenote_distance_m: f64,
    #[serde(default)]
    reference_station_max_m: f64,
}

#[derive(Debug, Deserialize)]
struct GeoJsonFeature {
    geometry: GeoJsonGeometry,
    properties: GeoJsonProperties,
}

#[derive(Debug, Deserialize)]
struct GeoJsonGeometry {
    coordinates: Vec<f64>,
}

#[derive(Debug, Deserialize, Default)]
struct GeoJsonAtom {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    severity: Option<u8>,
    #[serde(default)]
    style: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeoJsonProperties {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    stage: String,
    #[serde(default)]
    reference_track: String,
    #[serde(default)]
    note_index: usize,
    #[serde(default)]
    notes: Vec<String>,
    #[serde(default)]
    notes_text: String,
    #[serde(default)]
    link_to_next: bool,
    #[serde(default)]
    atoms: Vec<GeoJsonAtom>,
    #[serde(default)]
    turn_severity_min: Option<u8>,
}

fn corner_gear_severity_from_style(style: &str) -> Option<u8> {
    match style.to_ascii_lowercase().as_str() {
        "hp" | "acutehp" => Some(1),
        "square" | "chicane" | "chicaneentry" => Some(2),
        "openhp" => Some(2),
        "flat" => Some(3),
        "kink" => Some(5),
        _ => None,
    }
}

fn corner_gear_severity_from_token(token: &str) -> Option<u8> {
    let upper = token.to_ascii_uppercase();
    let rest = if let Some(rest) = upper.strip_prefix("LEFT") {
        rest
    } else if let Some(rest) = upper.strip_prefix("RIGHT") {
        rest
    } else {
        return None;
    };
    if rest.is_empty() {
        return None;
    }
    if let Some(first) = rest.chars().next() {
        if first.is_ascii_digit() {
            return first.to_digit(10).map(|value| value as u8);
        }
    }
    corner_gear_severity_from_style(rest)
}

fn corner_gear_severity_for_callout(
    notes: &[String],
    atoms: &[GeoJsonAtom],
    turn_severity_min: Option<u8>,
) -> Option<u8> {
    let mut values = Vec::new();
    if let Some(value) = turn_severity_min {
        values.push(value);
    }
    for token in notes {
        if let Some(value) = corner_gear_severity_from_token(token) {
            values.push(value);
        }
    }
    for atom in atoms {
        if atom.kind != "turn" {
            continue;
        }
        if let Some(value) = atom.severity {
            values.push(value);
        } else if let Some(style) = atom.style.as_deref() {
            if let Some(value) = corner_gear_severity_from_style(style) {
                values.push(value);
            }
        }
    }
    values.into_iter().min()
}

impl PacenoteStageCatalog {
    pub fn load_dir(dir: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut stages = Vec::new();
        if !dir.is_dir() {
            return Err(format!("Pacenote directory not found: {}", dir.display()).into());
        }
        for entry in std::fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("geojson") {
                continue;
            }
            let slug = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if let Some(summary) = load_stage_summary(&path, slug)? {
                stages.push(summary);
            }
        }
        stages.sort_by(|a, b| a.slug.cmp(&b.slug));
        Ok(Self { stages })
    }

    pub fn len(&self) -> usize {
        self.stages.len()
    }

    pub fn select_from_position(
        &self,
        x: f64,
        z: f64,
        radius_m: f64,
    ) -> Option<PacenoteStagePick> {
        let mut hits: Vec<&PacenoteStageSummary> = self
            .stages
            .iter()
            .filter(|stage| dist_xy((x, z), (stage.start_x, stage.start_z)) <= radius_m)
            .collect();
        if hits.is_empty() {
            return None;
        }
        let mut tracks: Vec<String> = hits
            .iter()
            .map(|stage| stage.reference_track.clone())
            .collect();
        tracks.sort();
        tracks.dedup();
        if tracks.len() != 1 {
            return None;
        }
        let reference_track = tracks.remove(0);
        hits.retain(|stage| stage.reference_track == reference_track);
        let best = pick_preferred_stage(&hits)?;
        Some(PacenoteStagePick {
            reference_track,
            slug: best.slug.clone(),
            path: best.path.clone(),
            stage: best.stage.clone(),
        })
    }

    /// Distance from player (game x, z) to each stage's first pacenote anchor (`start_x` / `start_z`).
    /// When `reference_filter` is set, only stages whose `reference_track` is in the set are included.
    /// Results sorted by ascending distance (nearest first).
    pub fn distances_to_first_anchors_sorted(
        &self,
        player_x: f64,
        player_z: f64,
        reference_filter: Option<&std::collections::HashSet<String>>,
    ) -> Vec<(f64, String, String)> {
        let mut out: Vec<(f64, String, String)> = self
            .stages
            .iter()
            .filter(|s| {
                reference_filter
                    .map_or(true, |names| names.contains(&s.reference_track))
            })
            .map(|s| {
                let d = dist_xy((player_x, player_z), (s.start_x, s.start_z));
                (d, s.slug.clone(), s.reference_track.clone())
            })
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }

    /// Stages whose **first** pacenote anchor lies within `radius_m` of the player (game x/z).
    /// Optional `locked_reference`: when set, only stages for that reference track name are kept.
    /// Optional `reference_filter`: when set, only stages whose `reference_track` is in the set.
    /// Returns `(distance_m, pick)` sorted by distance ascending.
    pub fn first_anchor_candidates_within(
        &self,
        player_x: f64,
        player_z: f64,
        radius_m: f64,
        reference_filter: Option<&HashSet<String>>,
        locked_reference: Option<&str>,
    ) -> Vec<(f64, PacenoteStagePick)> {
        let mut out: Vec<(f64, PacenoteStagePick)> = self
            .stages
            .iter()
            .filter(|s| {
                reference_filter.map_or(true, |names| names.contains(&s.reference_track))
            })
            .filter(|s| locked_reference.map_or(true, |lock| s.reference_track == lock))
            .filter_map(|s| {
                let d = dist_xy((player_x, player_z), (s.start_x, s.start_z));
                if d > radius_m {
                    return None;
                }
                Some((
                    d,
                    PacenoteStagePick {
                        reference_track: s.reference_track.clone(),
                        slug: s.slug.clone(),
                        path: s.path.clone(),
                        stage: s.stage.clone(),
                    },
                ))
            })
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }
}

fn load_stage_summary(
    path: &Path,
    slug: String,
) -> Result<Option<PacenoteStageSummary>, Box<dyn std::error::Error + Send + Sync>> {
    let raw = std::fs::read_to_string(path)?;
    let root: GeoJsonRoot = serde_json::from_str(&raw)?;
    let mut first: Option<&GeoJsonFeature> = None;
    for feature in &root.features {
        if feature.properties.kind != "pacenote" {
            continue;
        }
        match first {
            Some(current) if feature.properties.note_index >= current.properties.note_index => {}
            _ => first = Some(feature),
        }
    }
    let Some(feature) = first else {
        return Ok(None);
    };
    if feature.geometry.coordinates.len() < 2 {
        return Ok(None);
    }
    let (start_x, start_z) =
        gis::file_to_game_xz(feature.geometry.coordinates[0], feature.geometry.coordinates[1]);
    let max_distance_m = root
        .properties
        .max_pacenote_distance_m
        .max(root.properties.reference_station_max_m);
    Ok(Some(PacenoteStageSummary {
        slug,
        path: path.to_path_buf(),
        reference_track: feature.properties.reference_track.clone(),
        stage: feature.properties.stage.clone(),
        start_x,
        start_z,
        max_distance_m,
    }))
}

fn pick_preferred_stage<'a>(
    candidates: &[&'a PacenoteStageSummary],
) -> Option<&'a PacenoteStageSummary> {
    candidates.iter().copied().max_by(|a, b| {
        let a_full = stage_prefers_full(&a.slug, &a.stage);
        let b_full = stage_prefers_full(&b.slug, &b.stage);
        a_full
            .cmp(&b_full)
            .then_with(|| a.max_distance_m.total_cmp(&b.max_distance_m))
            .then_with(|| a.slug.cmp(&b.slug))
    })
}

fn stage_prefers_full(slug: &str, stage: &str) -> bool {
    let slug = slug.to_ascii_lowercase();
    let stage = stage.to_ascii_lowercase();
    slug.contains("full") || stage.contains("full")
}

pub fn load_course(path: &Path) -> Result<PacenoteCourse, Box<dyn std::error::Error + Send + Sync>> {
    let raw = std::fs::read_to_string(path)?;
    let root: GeoJsonRoot = serde_json::from_str(&raw)?;
    let mut callouts = Vec::new();
    let mut stage = String::new();
    let mut reference_track = String::new();
    for feature in root.features {
        if feature.properties.kind != "pacenote" {
            continue;
        }
        if feature.geometry.coordinates.len() < 2 {
            continue;
        }
        let (x, z) = gis::file_to_game_xz(feature.geometry.coordinates[0], feature.geometry.coordinates[1]);
        if stage.is_empty() {
            stage = feature.properties.stage.clone();
        }
        if reference_track.is_empty() {
            reference_track = feature.properties.reference_track.clone();
        }
        let notes = feature.properties.notes;
        let notes_text = if feature.properties.notes_text.is_empty() {
            notes.join(", ")
        } else {
            feature.properties.notes_text
        };
        callouts.push(PacenoteCallout {
            index: feature.properties.note_index,
            x,
            z,
            notes: notes.clone(),
            notes_text,
            link_to_next: feature.properties.link_to_next,
            max_turn_severity: corner_gear_severity_for_callout(
                &notes,
                &feature.properties.atoms,
                feature.properties.turn_severity_min,
            ),
        });
    }
    callouts.sort_by_key(|c| c.index);
    if callouts.is_empty() {
        return Err(format!("No pacenote features in {}", path.display()).into());
    }
    let leg_distance_m = leg_distances_m(&callouts);
    Ok(PacenoteCourse {
        stage,
        reference_track,
        callouts,
        leg_distance_m,
    })
}

impl PacenoteCourse {
    pub fn next_callout_pos(&self, triggered: &std::collections::HashSet<usize>) -> Option<usize> {
        self.callouts
            .iter()
            .position(|callout| !triggered.contains(&callout.index))
    }

    pub fn leg_distance_to_next(&self, pos: usize) -> f64 {
        self.leg_distance_m
            .get(pos)
            .copied()
            .unwrap_or(f64::INFINITY)
    }

    pub fn callout_chain_end_pos(&self, start: usize) -> usize {
        let mut pos = start;
        while self.callouts[pos].link_to_next && pos + 1 < self.callouts.len() {
            pos += 1;
        }
        pos
    }

    pub fn callout_chain_urgency(&self, start: usize, end: usize) -> u8 {
        let mut urgency = 6u8;
        for callout in &self.callouts[start..=end] {
            if let Some(severity) = callout.max_turn_severity {
                urgency = urgency.min(severity);
            }
        }
        urgency
    }

    pub fn route_distance_to_callout(&self, player: (f64, f64), pos: usize) -> f64 {
        if pos >= self.callouts.len() {
            return f64::INFINITY;
        }
        let callout = &self.callouts[pos];
        if pos == 0 {
            return dist_to_callout(player, callout);
        }
        let prev = &self.callouts[pos - 1];
        let (px, pz) = player;
        let (ax, az) = (prev.x, prev.z);
        let (bx, bz) = (callout.x, callout.z);
        let dx = bx - ax;
        let dz = bz - az;
        let len2 = dx * dx + dz * dz;
        if len2 <= 1e-9 {
            return dist_to_callout(player, callout);
        }
        let mut t = ((px - ax) * dx + (pz - az) * dz) / len2;
        t = t.clamp(0.0, 1.0);
        let proj_x = ax + t * dx;
        let proj_z = az + t * dz;
        dist_xy((px, pz), (proj_x, proj_z)) + dist_xy((proj_x, proj_z), (bx, bz))
    }
}

pub fn resolve_geojson_path(
    dir: &Path,
    reference_track: &str,
    stage_slug: Option<&str>,
) -> Option<PathBuf> {
    let ref_slug = normalize_track_slug(reference_track);
    if ref_slug.is_empty() {
        return None;
    }
    let mut matches: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("geojson") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            if stem.starts_with(&ref_slug) {
                matches.push(path);
            }
        }
    }
    matches.sort();

    match matches.len() {
        0 => {}
        1 => return Some(matches[0].clone()),
        _ => {
            if let Some(slug) = stage_slug {
                let explicit = dir.join(format!("{slug}.geojson"));
                if explicit.is_file() && matches.iter().any(|p| p == &explicit) {
                    return Some(explicit);
                }
                for m in &matches {
                    if m.file_stem().and_then(|s| s.to_str()) == Some(slug) {
                        return Some(m.clone());
                    }
                }
            }
            return Some(matches[0].clone());
        }
    }

    // No file named for this reference track: optional explicit stage file only if it
    // clearly belongs to the same track (avoids loading sisteron_mezien while on hafren_south).
    if let Some(slug) = stage_slug {
        let explicit = dir.join(format!("{slug}.geojson"));
        if explicit.is_file() {
            let stem = explicit.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let stem_n = normalize_track_slug(stem);
            if stem_n.starts_with(&ref_slug) || ref_slug.starts_with(&stem_n) {
                return Some(explicit);
            }
        }
    }
    None
}

pub fn normalize_track_slug(name: &str) -> String {
    let mut out = String::new();
    let mut prev_sep = true;
    for ch in name.chars() {
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

pub fn crossed_callout(
    from: (f64, f64),
    to: (f64, f64),
    callout: &PacenoteCallout,
    radius_m: f64,
) -> bool {
    let d0 = dist_xy(from, (callout.x, callout.z));
    let d1 = dist_xy(to, (callout.x, callout.z));
    d0 > radius_m && d1 <= radius_m
}

pub fn dist_to_callout(player: (f64, f64), callout: &PacenoteCallout) -> f64 {
    dist_xy(player, (callout.x, callout.z))
}

pub fn lead_distance_m(speed_kmh: f64, lead_sec: f64, extra_lead_sec: f64) -> f64 {
    let speed_m_s = speed_kmh.max(0.0) / 3.6;
    speed_m_s * (lead_sec.max(0.0) + extra_lead_sec.max(0.0))
}

pub fn driving_gear(acc_gear: i32) -> Option<u8> {
    if acc_gear <= 0 {
        return None;
    }
    Some((acc_gear - 1).clamp(1, 6) as u8)
}

pub fn gear_extra_lead_sec(
    driving_gear: u8,
    corner_gear: u8,
    reference_gear: u8,
    gear_step_ms: u64,
) -> f64 {
    if driving_gear <= corner_gear {
        return 0.0;
    }
    let steps = driving_gear.min(reference_gear).saturating_sub(corner_gear);
    steps as f64 * gear_step_ms as f64 / 1000.0
}

pub fn capped_lookahead_m(lookahead_m: f64, leg_to_next_m: f64, skip_buffer_m: f64) -> f64 {
    if !leg_to_next_m.is_finite() {
        return lookahead_m.max(0.0);
    }
    let max_lookahead = (leg_to_next_m - skip_buffer_m).max(0.0);
    lookahead_m.max(0.0).min(max_lookahead)
}

pub fn should_trigger_ahead(distance_m: f64, lookahead_m: f64) -> bool {
    distance_m <= lookahead_m
}

fn leg_distances_m(callouts: &[PacenoteCallout]) -> Vec<f64> {
    let mut out = Vec::with_capacity(callouts.len());
    for idx in 0..callouts.len() {
        if idx + 1 < callouts.len() {
            out.push(dist_xy(
                (callouts[idx].x, callouts[idx].z),
                (callouts[idx + 1].x, callouts[idx + 1].z),
            ));
        } else {
            out.push(f64::INFINITY);
        }
    }
    out
}

fn dist_xy(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = a.0 - b.0;
    let dz = a.1 - b.1;
    (dx * dx + dz * dz).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_geojson_prefers_track_prefix_over_unrelated_stage() {
        let dir = std::env::temp_dir().join(format!("acr_pn_resolve_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sisteron_mezien.geojson"), "{}").unwrap();
        std::fs::write(dir.join("hafren_south_x.geojson"), "{}").unwrap();
        let got = resolve_geojson_path(&dir, "hafren_south", Some("sisteron_mezien"));
        assert_eq!(got, Some(dir.join("hafren_south_x.geojson")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_geojson_rejects_unrelated_explicit_when_no_track_file() {
        let dir = std::env::temp_dir().join(format!("acr_pn_resolve2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sisteron_mezien.geojson"), "{}").unwrap();
        let got = resolve_geojson_path(&dir, "hafren_south", Some("sisteron_mezien"));
        assert_eq!(got, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_geojson_explicit_fallback_when_stem_matches_track() {
        let dir = std::env::temp_dir().join(format!("acr_pn_resolve3_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sisteron_mezien.geojson"), "{}").unwrap();
        let got = resolve_geojson_path(&dir, "sisteron", Some("sisteron_mezien"));
        assert_eq!(got, Some(dir.join("sisteron_mezien.geojson")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corner_gear_severity_covers_styles_and_numbers() {
        assert_eq!(corner_gear_severity_from_token("RightHP"), Some(1));
        assert_eq!(corner_gear_severity_from_token("LeftSquare"), Some(2));
        assert_eq!(corner_gear_severity_from_token("Right3"), Some(3));
        let atoms = vec![GeoJsonAtom {
            kind: "turn".into(),
            severity: None,
            style: Some("hp".into()),
        }];
        assert_eq!(
            corner_gear_severity_for_callout(&["RightHP".into()], &atoms, None),
            Some(1)
        );
        assert_eq!(
            corner_gear_severity_for_callout(&["RightHP".into(), "Right5".into()], &[], None),
            Some(1)
        );
    }

    #[test]
    fn gear_extra_lead_matches_corner_gap() {
        assert!((gear_extra_lead_sec(5, 2, 6, 300) - 0.9).abs() < 1e-9);
        assert!((gear_extra_lead_sec(6, 1, 6, 2000) - 10.0).abs() < 1e-9);
        assert_eq!(gear_extra_lead_sec(1, 1, 6, 300), 0.0);
    }

    #[test]
    fn route_distance_follows_pacenote_leg() {
        let course = PacenoteCourse {
            stage: "test".into(),
            reference_track: "test".into(),
            callouts: vec![
                PacenoteCallout {
                    index: 0,
                    x: 0.0,
                    z: 0.0,
                    notes: vec![],
                    notes_text: String::new(),
                    link_to_next: false,
                    max_turn_severity: None,
                },
                PacenoteCallout {
                    index: 1,
                    x: 100.0,
                    z: 0.0,
                    notes: vec![],
                    notes_text: String::new(),
                    link_to_next: false,
                    max_turn_severity: Some(1),
                },
            ],
            leg_distance_m: vec![100.0, f64::INFINITY],
        };
        assert!((course.route_distance_to_callout((50.0, 20.0), 1) - 70.0).abs() < 1e-6);
    }

    #[test]
    fn lookahead_is_capped_before_next_callout() {
        let capped = capped_lookahead_m(40.0, 30.0, 5.0);
        assert!((capped - 25.0).abs() < 1e-9);
    }

    #[test]
    fn prefers_full_then_longest_stage() {
        let short = PacenoteStageSummary {
            slug: "sisteron_mezien".into(),
            path: PathBuf::from("timing/pacenotes/sisteron_mezien.geojson"),
            reference_track: "sisteron".into(),
            stage: "Sisteron - Mézien".into(),
            start_x: 0.0,
            start_z: 0.0,
            max_distance_m: 7698.0,
        };
        let long = PacenoteStageSummary {
            slug: "sisteron_st_geniez".into(),
            path: PathBuf::from("timing/pacenotes/sisteron_st_geniez.geojson"),
            reference_track: "sisteron".into(),
            stage: "Sisteron - St. Geniez".into(),
            start_x: 0.0,
            start_z: 0.0,
            max_distance_m: 13711.0,
        };
        let full = PacenoteStageSummary {
            slug: "sisteron_full".into(),
            path: PathBuf::from("timing/pacenotes/sisteron_full.geojson"),
            reference_track: "sisteron".into(),
            stage: "Sisteron Full".into(),
            start_x: 0.0,
            start_z: 0.0,
            max_distance_m: 9000.0,
        };
        let picked = pick_preferred_stage(&[&short, &long, &full]).expect("pick stage");
        assert_eq!(picked.slug, "sisteron_full");
        let picked = pick_preferred_stage(&[&short, &long]).expect("pick stage");
        assert_eq!(picked.slug, "sisteron_st_geniez");
    }
}
