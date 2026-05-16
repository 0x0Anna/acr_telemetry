//! Calibrated stage sector markers (`timing/timing_sectors/{slug}.geojson` or filtered collection).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingSectorRole {
    TimingStart,
    SectorBoundary,
    Finish,
}

impl TimingSectorRole {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "timing_start" => Some(Self::TimingStart),
            "sector_boundary" => Some(Self::SectorBoundary),
            "finish" => Some(Self::Finish),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TimingStart => "timing_start",
            Self::SectorBoundary => "sector_boundary",
            Self::Finish => "finish",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TimingSectorMarker {
    pub role: TimingSectorRole,
    pub order: i32,
    pub label: String,
    pub x: f64,
    pub z: f64,
}

/// Perpendicular timing gate (game XZ) crossed when driving the stage.
#[derive(Debug, Clone, Copy)]
pub struct TimingGate {
    pub marker_index: usize,
    pub a: (f64, f64),
    pub b: (f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatePassMethod {
    GateLine,
    /// Only for `timing_start` (grid spawn inside start disc).
    RadiusDisc,
}

#[derive(Debug, Clone)]
pub struct StageTimingSectors {
    pub stage_slug: String,
    pub reference_track: String,
    pub markers: Vec<TimingSectorMarker>,
    pub gates: Vec<TimingGate>,
    /// Number of timed sector legs (timing_start→S1, S1→S2, …, S3→finish).
    pub sector_leg_count: usize,
}

fn finish_stage_timing_sectors(
    stage_slug: String,
    reference_track: String,
    markers: Vec<TimingSectorMarker>,
    gate_half_width_m: f64,
) -> StageTimingSectors {
    let sector_leg_count = sector_leg_count_from_markers(&markers);
    let gates = build_timing_gates(&markers, gate_half_width_m);
    StageTimingSectors {
        stage_slug,
        reference_track,
        markers,
        gates,
        sector_leg_count,
    }
}

/// Build a gate line perpendicular to the local route at each marker.
pub fn build_timing_gates(markers: &[TimingSectorMarker], half_width_m: f64) -> Vec<TimingGate> {
    let half_width_m = half_width_m.max(20.0);
    let n = markers.len();
    let mut gates = Vec::with_capacity(n);
    for i in 0..n {
        let m = &markers[i];
        let (dx, dz) = if i + 1 < n {
            (
                markers[i + 1].x - m.x,
                markers[i + 1].z - m.z,
            )
        } else if i > 0 {
            (m.x - markers[i - 1].x, m.z - markers[i - 1].z)
        } else {
            (1.0, 0.0)
        };
        let len = (dx * dx + dz * dz).sqrt().max(1e-6);
        let px = -dz / len;
        let pz = dx / len;
        gates.push(TimingGate {
            marker_index: i,
            a: (m.x + px * half_width_m, m.z + pz * half_width_m),
            b: (m.x - px * half_width_m, m.z - pz * half_width_m),
        });
    }
    gates
}

#[derive(Debug, Deserialize)]
struct CollectionProps {
    stage_slug: Option<String>,
    reference_track: Option<String>,
    coordinate_space: Option<String>,
}

fn geometry_to_game_xz(coordinate_space: Option<&str>, c0: f64, c1: f64) -> (f64, f64) {
    match coordinate_space.unwrap_or("acc_world_zx") {
        "acc_world_xz" | "game" => (c0, c1),
        _ => acr_telemetry::gis::file_to_game_xz(c0, c1),
    }
}

#[derive(Debug, Deserialize)]
struct FeatureProps {
    marker_role: String,
    marker_order: Option<i32>,
    marker_label: Option<String>,
    game_x: Option<f64>,
    game_z: Option<f64>,
}

pub fn default_sectors_dir() -> PathBuf {
    PathBuf::from("timing/timing_sectors")
}

pub fn path_for_slug(sectors_dir: &Path, stage_slug: &str) -> PathBuf {
    sectors_dir.join(format!("{stage_slug}.geojson"))
}

pub fn default_path_for_slug(stage_slug: &str) -> PathBuf {
    path_for_slug(&default_sectors_dir(), stage_slug)
}

pub fn legacy_collection_path() -> PathBuf {
    PathBuf::from("timing/timing_sectors.geojson")
}

pub fn load(path: &Path) -> Result<StageTimingSectors, Box<dyn std::error::Error>> {
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
            coordinate_space: None,
        });

    let stage_slug = coll
        .stage_slug
        .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().to_string());
    let reference_track = coll.reference_track.unwrap_or_default();
    let coordinate_space = coll.coordinate_space.as_deref();

    let mut markers = Vec::new();
    if let Some(features) = root.get("features").and_then(|v| v.as_array()) {
        for f in features {
            let props_val = f.get("properties").cloned().unwrap_or(Value::Null);
            let props: FeatureProps = serde_json::from_value(props_val)?;
            let role = TimingSectorRole::parse(props.marker_role.trim())
                .ok_or_else(|| format!("unknown marker_role: {}", props.marker_role))?;
            let (x, z) = if let (Some(gx), Some(gz)) = (props.game_x, props.game_z) {
                (gx, gz)
            } else {
                let coords = f
                    .get("geometry")
                    .and_then(|g| g.get("coordinates"))
                    .and_then(|c| c.as_array())
                    .ok_or("marker missing Point coordinates")?;
                if coords.len() < 2 {
                    continue;
                }
                let c0 = coords[0].as_f64().ok_or("coordinate x not f64")?;
                let c1 = coords[1].as_f64().ok_or("coordinate y not f64")?;
                geometry_to_game_xz(coordinate_space, c0, c1)
            };
            markers.push(TimingSectorMarker {
                role,
                order: props.marker_order.unwrap_or(0),
                label: props
                    .marker_label
                    .unwrap_or_else(|| format!("order_{}", props.marker_order.unwrap_or(0))),
                x,
                z,
            });
        }
    }
    markers.sort_by_key(|m| m.order);
    Ok(finish_stage_timing_sectors(
        stage_slug,
        reference_track,
        markers,
        50.0,
    ))
}

pub fn load_filtered_from_collection(
    path: &Path,
    stage_slug: &str,
) -> Result<StageTimingSectors, Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string(path)?;
    let root: Value = serde_json::from_str(&raw)?;
    let coll_slug = root
        .get("properties")
        .and_then(|p| p.get("stage_slug"))
        .and_then(|v| v.as_str());
    if coll_slug == Some(stage_slug) {
        return load(path);
    }
    let coll: CollectionProps = root
        .get("properties")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or(CollectionProps {
            stage_slug: None,
            reference_track: None,
            coordinate_space: None,
        });
    let coordinate_space = coll.coordinate_space.as_deref();
    let mut markers = Vec::new();
    if let Some(features) = root.get("features").and_then(|v| v.as_array()) {
        for f in features {
            let props = f.get("properties").cloned().unwrap_or(Value::Null);
            let feat_slug = props.get("stage_slug").and_then(|v| v.as_str());
            if feat_slug != Some(stage_slug) {
                continue;
            }
            let props: FeatureProps = serde_json::from_value(props)?;
            let role = TimingSectorRole::parse(props.marker_role.trim())
                .ok_or_else(|| format!("unknown marker_role: {}", props.marker_role))?;
            let (x, z) = if let (Some(gx), Some(gz)) = (props.game_x, props.game_z) {
                (gx, gz)
            } else {
                let coords = f
                    .get("geometry")
                    .and_then(|g| g.get("coordinates"))
                    .and_then(|c| c.as_array())
                    .ok_or("marker missing Point coordinates")?;
                if coords.len() < 2 {
                    continue;
                }
                let c0 = coords[0].as_f64().ok_or("coordinate x not f64")?;
                let c1 = coords[1].as_f64().ok_or("coordinate y not f64")?;
                geometry_to_game_xz(coordinate_space, c0, c1)
            };
            markers.push(TimingSectorMarker {
                role,
                order: props.marker_order.unwrap_or(0),
                label: props
                    .marker_label
                    .unwrap_or_else(|| format!("order_{}", props.marker_order.unwrap_or(0))),
                x,
                z,
            });
        }
    }
    if markers.is_empty() {
        return Err(format!("no timing sectors for stage_slug {stage_slug}").into());
    }
    markers.sort_by_key(|m| m.order);
    let reference_track = root
        .get("properties")
        .and_then(|p| p.get("reference_track"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(finish_stage_timing_sectors(
        stage_slug.to_string(),
        reference_track,
        markers,
        50.0,
    ))
}

pub type SectorCache = HashMap<String, StageTimingSectors>;

/// Load calibrated sectors for a stage slug (timing-only; not tied to pacenotes).
pub fn load_for_stage_slug<'a>(
    stage_slug: &str,
    sectors_dir: &Path,
    cache: &'a mut SectorCache,
) -> Option<&'a StageTimingSectors> {
    if !cache.contains_key(stage_slug) {
        let per_slug = path_for_slug(sectors_dir, stage_slug);
        let loaded = if per_slug.exists() {
            load(&per_slug).ok()
        } else {
            let legacy = legacy_collection_path();
            if legacy.exists() {
                load_filtered_from_collection(&legacy, stage_slug).ok()
            } else {
                None
            }
        };
        if let Some(m) = loaded {
            cache.insert(stage_slug.to_string(), m);
        } else {
            return None;
        }
    }
    cache.get(stage_slug)
}

/// Timed legs: every marker after `timing_start` (sector boundaries + finish).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sector_boundary_does_not_use_radius_fallback() {
        let markers = vec![
            TimingSectorMarker {
                role: TimingSectorRole::TimingStart,
                order: 0,
                label: "Start".into(),
                x: 0.0,
                z: 0.0,
            },
            TimingSectorMarker {
                role: TimingSectorRole::SectorBoundary,
                order: 1,
                label: "S1".into(),
                x: 0.0,
                z: 100.0,
            },
        ];
        let gates = build_timing_gates(&markers, 50.0);
        let m = &markers[1];
        // 40 m before marker along track (same as live S1 case: z 272 vs 312).
        assert!(
            passes_timing_gate_method((0.0, 59.0), (0.0, 61.0), 1, m, &gates, 40.0).is_none()
        );
    }

    #[test]
    fn gate_line_is_crossed_by_segment() {
        let markers = vec![
            TimingSectorMarker {
                role: TimingSectorRole::TimingStart,
                order: 0,
                label: "Start".into(),
                x: 0.0,
                z: 0.0,
            },
            TimingSectorMarker {
                role: TimingSectorRole::SectorBoundary,
                order: 1,
                label: "S1".into(),
                x: 100.0,
                z: 0.0,
            },
        ];
        let gates = build_timing_gates(&markers, 50.0);
        assert_eq!(gates.len(), 2);
        let g = &gates[1];
        assert!(crossed_timing_gate((90.0, -5.0), (110.0, 5.0), g));
        assert!(!crossed_timing_gate((10.0, 0.0), (20.0, 0.0), g));
    }

    #[test]
    fn leg_count_includes_finish() {
        let markers = vec![
            TimingSectorMarker {
                role: TimingSectorRole::TimingStart,
                order: 0,
                label: "Start".into(),
                x: 0.0,
                z: 0.0,
            },
            TimingSectorMarker {
                role: TimingSectorRole::SectorBoundary,
                order: 1,
                label: "S1".into(),
                x: 1.0,
                z: 0.0,
            },
            TimingSectorMarker {
                role: TimingSectorRole::SectorBoundary,
                order: 2,
                label: "S2".into(),
                x: 2.0,
                z: 0.0,
            },
            TimingSectorMarker {
                role: TimingSectorRole::SectorBoundary,
                order: 3,
                label: "S3".into(),
                x: 3.0,
                z: 0.0,
            },
            TimingSectorMarker {
                role: TimingSectorRole::Finish,
                order: 4,
                label: "Finish".into(),
                x: 4.0,
                z: 0.0,
            },
        ];
        assert_eq!(sector_leg_count_from_markers(&markers), 4);
    }
}

pub fn sector_leg_count_from_markers(markers: &[TimingSectorMarker]) -> usize {
    markers
        .iter()
        .filter(|m| m.role != TimingSectorRole::TimingStart)
        .count()
        .max(1)
}

pub fn dist_xz(ax: f64, az: f64, bx: f64, bz: f64) -> f64 {
    let dx = ax - bx;
    let dz = az - bz;
    (dx * dx + dz * dz).sqrt()
}

/// Radius crossing (same rule as pacenote `crossed_callout`).
pub fn crossed_marker(
    from: (f64, f64),
    to: (f64, f64),
    marker: &TimingSectorMarker,
    radius_m: f64,
) -> bool {
    let d0 = dist_xz(from.0, from.1, marker.x, marker.z);
    let d1 = dist_xz(to.0, to.1, marker.x, marker.z);
    d0 > radius_m && d1 <= radius_m
}

fn dist_point_to_segment(px: f64, pz: f64, ax: f64, az: f64, bx: f64, bz: f64) -> f64 {
    let abx = bx - ax;
    let abz = bz - az;
    let len_sq = abx * abx + abz * abz;
    if len_sq < 1e-12 {
        return dist_xz(px, pz, ax, az);
    }
    let t = ((px - ax) * abx + (pz - az) * abz) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let cx = ax + t * abx;
    let cz = az + t * abz;
    dist_xz(px, pz, cx, cz)
}

/// True if the movement segment enters the marker disc (crossing or pass-through).
pub fn passes_marker(
    from: (f64, f64),
    to: (f64, f64),
    marker: &TimingSectorMarker,
    radius_m: f64,
) -> bool {
    if crossed_marker(from, to, marker, radius_m) {
        return true;
    }
    dist_point_to_segment(marker.x, marker.z, from.0, from.1, to.0, to.1) <= radius_m
}

fn segment_intersection_t(
    p0: (f64, f64),
    p1: (f64, f64),
    q0: (f64, f64),
    q1: (f64, f64),
) -> Option<f64> {
    let r = (p1.0 - p0.0, p1.1 - p0.1);
    let s = (q1.0 - q0.0, q1.1 - q0.1);
    let rxs = r.0 * s.1 - r.1 * s.0;
    if rxs.abs() < 1e-9 {
        return None;
    }
    let qp = (q0.0 - p0.0, q0.1 - p0.1);
    let t = (qp.0 * s.1 - qp.1 * s.0) / rxs;
    let u = (qp.0 * r.1 - qp.1 * r.0) / rxs;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
        Some(t)
    } else {
        None
    }
}

/// True if the drive segment crosses the timing gate line.
pub fn crossed_timing_gate(from: (f64, f64), to: (f64, f64), gate: &TimingGate) -> bool {
    segment_intersection_t(from, to, gate.a, gate.b).is_some()
}

/// How the car triggered this marker (if at all).
pub fn passes_timing_gate_method(
    from: (f64, f64),
    to: (f64, f64),
    marker_index: usize,
    marker: &TimingSectorMarker,
    gates: &[TimingGate],
    radius_m: f64,
) -> Option<GatePassMethod> {
    if gates
        .iter()
        .any(|g| g.marker_index == marker_index && crossed_timing_gate(from, to, g))
    {
        return Some(GatePassMethod::GateLine);
    }
    // Sector/finish: gate line only — radius would fire ~radius_m early along the stage.
    if marker.role == TimingSectorRole::TimingStart && passes_marker(from, to, marker, radius_m) {
        return Some(GatePassMethod::RadiusDisc);
    }
    None
}

/// Gate line first; radius disc only for `timing_start`.
pub fn passes_timing_gate(
    from: (f64, f64),
    to: (f64, f64),
    marker_index: usize,
    marker: &TimingSectorMarker,
    gates: &[TimingGate],
    radius_m: f64,
) -> bool {
    passes_timing_gate_method(from, to, marker_index, marker, gates, radius_m).is_some()
}
