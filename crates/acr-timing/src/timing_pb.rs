//! Personal-best split times in a human-readable TOML file (`timing_pb.toml`).
//!
//! Deltas (OSD, beeps, cumulative pace) use this store. `timing.db` keeps every attempt
//! for correlation / blame analysis.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::timing_db::SplitRecord;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LegKey {
    track: String,
    car: String,
    direction: String,
    from: i32,
    to: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PbLegEntry {
    track: String,
    car: String,
    direction: String,
    from: i32,
    to: i32,
    duration_sec: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated_utc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    distance_m: Option<f64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PbFile {
    #[serde(default)]
    legs: Vec<PbLegEntry>,
}

/// In-memory PB table backed by `timing_pb.toml`.
#[derive(Debug)]
pub struct TimingPbStore {
    path: PathBuf,
    index: HashMap<LegKey, PbLegEntry>,
}

impl TimingPbStore {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let index = if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            let file: PbFile = toml::from_str(&raw)?;
            file.legs.into_iter().map(|e| (leg_key_from_entry(&e), e)).collect()
        } else {
            HashMap::new()
        };
        Ok(Self {
            path: path.to_path_buf(),
            index,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn best_time(
        &self,
        track_name: &str,
        car_model: &str,
        direction: &str,
        from_sector: i32,
        to_sector: i32,
    ) -> Option<f64> {
        self.index
            .get(&LegKey {
                track: track_name.to_string(),
                car: car_model.to_string(),
                direction: direction.to_string(),
                from: from_sector,
                to: to_sector,
            })
            .map(|e| e.duration_sec)
    }

    /// Δ vs PB for beeps/OSD. Returns `None` if PB is missing or clearly bogus (wrong gate / import).
    pub fn leg_delta_for_feedback(
        &self,
        track_name: &str,
        car_model: &str,
        direction: &str,
        from_sector: i32,
        to_sector: i32,
        duration_sec: f64,
    ) -> Option<f64> {
        let pb = self.best_time(track_name, car_model, direction, from_sector, to_sector)?;
        if duration_sec < 0.05 || pb < 0.05 {
            return None;
        }
        let delta = duration_sec - pb;
        let ratio = duration_sec / pb;
        // Only reject extreme mismatches (e.g. timer not anchored at Start, wrong leg id).
        if ratio > 4.0 || ratio < 0.25 {
            return None;
        }
        if delta.abs() > 20.0 {
            return None;
        }
        Some(delta)
    }

    /// Sum of PB leg times; `None` if any leg in the chain has no PB yet.
    pub fn cumulative_best_time(
        &self,
        track_name: &str,
        car_model: &str,
        direction: &str,
        legs: &[(i32, i32)],
    ) -> Option<f64> {
        if legs.is_empty() {
            return None;
        }
        let mut sum = 0.0f64;
        for &(from, to) in legs {
            sum += self.best_time(track_name, car_model, direction, from, to)?;
        }
        Some(sum)
    }

    /// PB before this split; updates file when `rec` is faster (or new).
    pub fn best_before_and_maybe_update(
        &mut self,
        rec: &SplitRecord<'_>,
    ) -> Result<Option<f64>, Box<dyn std::error::Error>> {
        let key = leg_key_from_record(rec);
        let best_before = self.index.get(&key).map(|e| e.duration_sec);
        // Ignore clearly bogus Start→CP1 (gate spam before timer armed).
        if rec.from_sector == 0 && rec.duration_sec < 3.0 {
            return Ok(best_before);
        }
        let improved = best_before.map_or(true, |b| rec.duration_sec < b - 1e-6);
        if improved {
            let entry = PbLegEntry {
                track: rec.track_name.to_string(),
                car: rec.car_model.to_string(),
                direction: rec.direction.to_string(),
                from: rec.from_sector,
                to: rec.to_sector,
                duration_sec: rec.duration_sec,
                updated_utc: Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()),
                distance_m: Some(rec.distance_m),
            };
            self.index.insert(key, entry);
            self.save()?;
            eprintln!(
                "timing_pb: new PB [{}]→[{}] {} {:.3}s",
                rec.from_sector, rec.to_sector, rec.track_name, rec.duration_sec
            );
        }
        Ok(best_before)
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut legs: Vec<PbLegEntry> = self.index.values().cloned().collect();
        legs.sort_by(|a, b| {
            (&a.track, &a.car, &a.direction, a.from, a.to).cmp(&(
                &b.track,
                &b.car,
                &b.direction,
                b.from,
                b.to,
            ))
        });
        let file = PbFile { legs };
        let raw = format!(
            "# Personal-best split times (seconds). Used for Δ on OSD / beeps.\n# All runs are still logged in timing.db.\n\n{}",
            toml::to_string_pretty(&file)?
        );
        std::fs::write(&self.path, raw)?;
        Ok(())
    }

    /// Seed PB file from `MIN(duration_sec)` per leg in `timing.db` (one-time / repair).
    pub fn import_from_db(&mut self, conn: &Connection) -> Result<usize, Box<dyn std::error::Error>> {
        let mut stmt = conn.prepare(
            r#"
SELECT track_name, car_model, direction, from_sector, to_sector,
       MIN(duration_sec), MIN(distance_m)
FROM sector_splits
GROUP BY track_name, car_model, direction, from_sector, to_sector
"#,
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i32>(3)?,
                r.get::<_, i32>(4)?,
                r.get::<_, f64>(5)?,
                r.get::<_, Option<f64>>(6)?,
            ))
        })?;
        let mut n = 0usize;
        for row in rows {
            let (track, car, direction, from, to, sec, dist) = row?;
            let entry = PbLegEntry {
                track,
                car,
                direction,
                from,
                to,
                duration_sec: sec,
                updated_utc: None,
                distance_m: dist,
            };
            self.index.insert(leg_key_from_entry(&entry), entry);
            n += 1;
        }
        if n > 0 {
            self.save()?;
        }
        Ok(n)
    }
}

fn leg_key_from_entry(e: &PbLegEntry) -> LegKey {
    LegKey {
        track: e.track.clone(),
        car: e.car.clone(),
        direction: e.direction.clone(),
        from: e.from,
        to: e.to,
    }
}

fn leg_key_from_record(rec: &SplitRecord<'_>) -> LegKey {
    LegKey {
        track: rec.track_name.to_string(),
        car: rec.car_model.to_string(),
        direction: rec.direction.to_string(),
        from: rec.from_sector,
        to: rec.to_sector,
    }
}
