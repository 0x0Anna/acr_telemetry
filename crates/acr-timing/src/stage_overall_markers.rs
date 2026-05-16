//! Stage start / finish / sector points for overall (Gesamt-) timing from pacenote exports.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverallMarkerRole {
    Start,
    Finish,
    Sector,
}

impl OverallMarkerRole {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "start" => Some(Self::Start),
            "finish" => Some(Self::Finish),
            "sector" => Some(Self::Sector),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OverallMarker {
    pub role: OverallMarkerRole,
    pub order: i32,
    pub x: f64,
    pub z: f64,
    pub distance_lookup_m: f64,
    pub sector_label: Option<String>,
    pub notes_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StageOverallMarkers {
    pub stage_slug: String,
    pub reference_track: String,
    pub start_distance_lookup_m: f64,
    pub finish_distance_lookup_m: f64,
    pub overall_route_lookup_m: f64,
    pub markers: Vec<OverallMarker>,
}

#[derive(Debug, Deserialize)]
struct CollectionProps {
    stage_slug: Option<String>,
    reference_track: Option<String>,
    start_distance_lookup_m: Option<f64>,
    finish_distance_lookup_m: Option<f64>,
    overall_route_lookup_m: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct FeatureProps {
    marker_role: String,
    marker_order: Option<i32>,
    distance_lookup_m: Option<f64>,
    sector_label: Option<String>,
    notes_text: Option<String>,
}

/// Default search path: `timing/overall_markers/{stage_slug}.geojson`.
pub fn default_path_for_slug(stage_slug: &str) -> PathBuf {
    PathBuf::from("timing/overall_markers").join(format!("{stage_slug}.geojson"))
}

pub fn load(path: &Path) -> Result<StageOverallMarkers, Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string(path)?;
    let root: Value = serde_json::from_str(&raw)?;
    let coll: CollectionProps = root
        .get("properties")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or(CollectionProps {
            stage_slug: None,
            reference_track: None,
            start_distance_lookup_m: None,
            finish_distance_lookup_m: None,
            overall_route_lookup_m: None,
        });

    let stage_slug = coll
        .stage_slug
        .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().to_string());
    let reference_track = coll.reference_track.unwrap_or_default();

    let mut markers = Vec::new();
    if let Some(features) = root.get("features").and_then(|v| v.as_array()) {
        for f in features {
            let props_val = f.get("properties").cloned().unwrap_or(Value::Null);
            let props: FeatureProps = serde_json::from_value(props_val)?;
            let role = OverallMarkerRole::parse(props.marker_role.trim())
                .ok_or_else(|| format!("unknown marker_role: {}", props.marker_role))?;
            let coords = f
                .get("geometry")
                .and_then(|g| g.get("coordinates"))
                .and_then(|c| c.as_array())
                .ok_or("marker missing Point coordinates")?;
            if coords.len() < 2 {
                continue;
            }
            let file_x = coords[0].as_f64().ok_or("coordinate x not f64")?;
            let file_y = coords[1].as_f64().ok_or("coordinate y not f64")?;
            let (x, z) = acr_telemetry::gis::file_to_game_xz(file_x, file_y);
            markers.push(OverallMarker {
                role,
                order: props.marker_order.unwrap_or(0),
                x,
                z,
                distance_lookup_m: props.distance_lookup_m.unwrap_or(0.0),
                sector_label: props.sector_label,
                notes_text: props.notes_text,
            });
        }
    }
    markers.sort_by_key(|m| m.order);

    let start_distance_lookup_m = coll.start_distance_lookup_m.unwrap_or_else(|| {
        markers
            .iter()
            .find(|m| m.role == OverallMarkerRole::Start)
            .map(|m| m.distance_lookup_m)
            .unwrap_or(0.0)
    });
    let finish_distance_lookup_m = coll.finish_distance_lookup_m.unwrap_or_else(|| {
        markers
            .iter()
            .find(|m| m.role == OverallMarkerRole::Finish)
            .map(|m| m.distance_lookup_m)
            .unwrap_or(0.0)
    });
    let overall_route_lookup_m = coll.overall_route_lookup_m.unwrap_or_else(|| {
        (finish_distance_lookup_m - start_distance_lookup_m).max(0.0)
    });

    Ok(StageOverallMarkers {
        stage_slug,
        reference_track,
        start_distance_lookup_m,
        finish_distance_lookup_m,
        overall_route_lookup_m,
        markers,
    })
}

pub fn try_load_slug(stage_slug: &str) -> Option<StageOverallMarkers> {
    let path = default_path_for_slug(stage_slug);
    load(&path).ok()
}

pub fn start_finish(
    markers: &StageOverallMarkers,
) -> Option<(&OverallMarker, &OverallMarker)> {
    let start = markers
        .markers
        .iter()
        .find(|m| m.role == OverallMarkerRole::Start)?;
    let finish = markers
        .markers
        .iter()
        .find(|m| m.role == OverallMarkerRole::Finish)?;
    Some((start, finish))
}

pub fn sectors(markers: &StageOverallMarkers) -> impl Iterator<Item = &OverallMarker> {
    markers
        .markers
        .iter()
        .filter(|m| m.role == OverallMarkerRole::Sector)
}

/// Map pacenote GeoJSON path stem → loaded markers (cached per slug).
pub type MarkerCache = HashMap<String, StageOverallMarkers>;

pub fn load_for_pacenote_geojson<'a>(
    pacenote_geojson: &Path,
    cache: &'a mut MarkerCache,
) -> Option<&'a StageOverallMarkers> {
    let slug = pacenote_geojson.file_stem()?.to_string_lossy().to_string();
    if !cache.contains_key(&slug) {
        let path = default_path_for_slug(&slug);
        if let Ok(m) = load(&path) {
            cache.insert(slug.clone(), m);
        } else {
            return None;
        }
    }
    cache.get(&slug)
}
