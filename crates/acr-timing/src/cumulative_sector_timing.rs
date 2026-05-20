//! Cumulative subsection splits from calibrated GeoJSON gates (not SHP ring).

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crate::cumulative_timing_config::CumulativeTimingConfig;
use crate::timing_sectors::{self, StageTimingSectors, TimingSectorRole};

#[derive(Debug, Clone)]
pub struct CumulativeTrackSectors {
    pub slug: String,
    pub reference_track: String,
    pub sectors: Arc<StageTimingSectors>,
    /// `seg_id` per marker index (falls back to `marker.order`).
    pub seg_ids: Vec<i32>,
}

#[derive(Debug, Clone, Copy)]
pub struct CumulativeLegCross {
    pub from_gate_ix: usize,
    pub to_gate_ix: usize,
    pub from_seg: i32,
    pub to_seg: i32,
}

#[derive(Debug)]
pub struct CumulativeLegState {
    pub track: CumulativeTrackSectors,
    pub last_gate_ix: Option<usize>,
    gate_cooldown_until: Vec<Option<Instant>>,
}

impl CumulativeLegState {
    pub fn new(track: CumulativeTrackSectors) -> Self {
        let n = track.sectors.markers.len();
        Self {
            track,
            last_gate_ix: None,
            gate_cooldown_until: vec![None; n],
        }
    }

    /// True if the most recently crossed gate is the timing start (first anchor).
    pub fn last_gate_is_timing_start(&self) -> bool {
        self.last_gate_ix
            .and_then(|ix| self.track.sectors.markers.get(ix))
            .is_some_and(|m| m.role == TimingSectorRole::TimingStart)
    }

    /// Subsector CP (not a main-sector line like `Sector 2` / `Finish`).
    pub fn destination_is_silent_cp(&self, to_gate_ix: usize) -> bool {
        self.track
            .sectors
            .markers
            .get(to_gate_ix)
            .map(|m| {
                let label = m.label.trim();
                !label.starts_with("Sector ") && label != "Finish"
            })
            .unwrap_or(true)
    }

    pub fn seg_id(&self, gate_ix: usize) -> i32 {
        self.track
            .seg_ids
            .get(gate_ix)
            .copied()
            .unwrap_or_else(|| self.track.sectors.markers.get(gate_ix).map(|m| m.order).unwrap_or(gate_ix as i32))
    }

    /// Detect forward gate crossing along marker order (GeoJSON gate lines).
    pub fn observe_segment(
        &mut self,
        from: (f64, f64),
        to: (f64, f64),
        _total_drive_m: f64,
        radius_m: f64,
        cross_cooldown: std::time::Duration,
        now: Instant,
        debug: bool,
    ) -> Option<CumulativeLegCross> {
        let markers = &self.track.sectors.markers;
        let gates = &self.track.sectors.gates;
        let prev_ix = self.last_gate_ix;
        let mut crossed_ix: Option<usize> = None;
        for (i, marker) in markers.iter().enumerate() {
            if self
                .gate_cooldown_until
                .get(i)
                .and_then(|o| *o)
                .map_or(false, |until| now < until)
            {
                continue;
            }
            if timing_sectors::passes_timing_gate(from, to, i, marker, gates, radius_m) {
                let forward_ok = match prev_ix {
                    None => true,
                    Some(p) => i > p,
                };
                if forward_ok {
                    crossed_ix = Some(crossed_ix.map_or(i, |best| best.min(i)));
                }
            }
        }
        let to_ix = crossed_ix?;
        self.gate_cooldown_until[to_ix] = Some(now + cross_cooldown);

        if prev_ix == Some(to_ix) {
            return None;
        }

        let from_ix = match prev_ix {
            None => {
                self.last_gate_ix = Some(to_ix);
                eprintln!(
                    "cumulative: gate [{}] ({})",
                    self.seg_id(to_ix),
                    markers[to_ix].label
                );
                return None;
            }
            Some(p) if to_ix > p => p,
            Some(p) if to_ix < p => {
                eprintln!(
                    "cumulative: reverse cross [{}] after [{}] — ignored",
                    self.seg_id(to_ix),
                    self.seg_id(p)
                );
                return None;
            }
            _ => return None,
        };

        let leg = CumulativeLegCross {
            from_gate_ix: from_ix,
            to_gate_ix: to_ix,
            from_seg: self.seg_id(from_ix),
            to_seg: self.seg_id(to_ix),
        };
        self.last_gate_ix = Some(to_ix);
        Some(leg)
    }
}

pub fn load_track(
    cfg: &CumulativeTimingConfig,
    reference_track: &str,
    slug: &str,
) -> Result<CumulativeTrackSectors, Box<dyn std::error::Error>> {
    let path = timing_sectors::resolve_cumulative_sectors_path(&cfg.sectors_dir(), slug);
    let sectors = timing_sectors::load(&path)?;
    let ref_norm = crate::stage_timing_config::normalize_track_slug(reference_track);
    let file_ref = crate::stage_timing_config::normalize_track_slug(&sectors.reference_track);
    if !file_ref.is_empty() && file_ref != ref_norm {
        eprintln!(
            "cumulative {}: reference_track={} (config expects {})",
            path.display(),
            sectors.reference_track,
            reference_track
        );
    }
    let seg_ids = seg_ids_from_geojson(&path)?;
    Ok(CumulativeTrackSectors {
        slug: slug.to_string(),
        reference_track: reference_track.to_string(),
        sectors: Arc::new(sectors),
        seg_ids,
    })
}

fn seg_ids_from_geojson(path: &Path) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string(path)?;
    let root: serde_json::Value = serde_json::from_str(&raw)?;
    let mut items: Vec<(i32, i32)> = Vec::new();
    if let Some(features) = root.get("features").and_then(|v| v.as_array()) {
        for f in features {
            let props = f.get("properties").cloned().unwrap_or(serde_json::Value::Null);
            let order = props
                .get("marker_order")
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32;
            let seg = props
                .get("seg_id")
                .and_then(|v| v.as_i64())
                .map(|n| n as i32)
                .or_else(|| {
                    props
                        .get("sector_id")
                        .and_then(|v| v.as_i64())
                        .map(|n| n as i32)
                })
                .unwrap_or(order);
            items.push((order, seg));
        }
    }
    items.sort_by_key(|(o, _)| *o);
    Ok(items.into_iter().map(|(_, s)| s).collect())
}

pub fn load_all(
    cfg: &CumulativeTimingConfig,
) -> Result<std::collections::HashMap<String, CumulativeTrackSectors>, Box<dyn std::error::Error>> {
    let mut out = std::collections::HashMap::new();
    for (ref_track, slug) in &cfg.ref_track_sectors {
        let path = timing_sectors::resolve_cumulative_sectors_path(&cfg.sectors_dir(), slug);
        let track = load_track(cfg, ref_track, slug)?;
        let key = crate::stage_timing_config::normalize_track_slug(ref_track);
        let gate_mode = if timing_sectors::cumulative_sectors_use_linestrings(&path) {
            "LineString gate lines"
        } else {
            "synthetic perpendicular gates"
        };
        eprintln!(
            "cumulative timing: {} → {} ({} gates, {}, SHP subsection disabled)",
            ref_track,
            path.display(),
            track.sectors.markers.len(),
            gate_mode
        );
        out.insert(key, track);
    }
    Ok(out)
}
