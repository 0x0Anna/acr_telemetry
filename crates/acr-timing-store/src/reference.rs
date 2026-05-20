use std::path::Path;

use rusqlite::{params, Connection};

use crate::reference_mode::ReferenceTimeMode;
use crate::schema;

/// Frozen reference for one main sector (from fastest complete prior run).
#[derive(Debug, Clone)]
pub struct ReferenceSnapshot {
    pub run_id: i64,
    pub sector_index: u32,
    pub sub_ids: Vec<i32>,
    pub sub_times_sec: Vec<f64>,
    pub tot_sec: f64,
}

#[derive(Debug, Clone)]
pub struct ReferenceRun {
    pub id: i64,
    pub tot_sec: f64,
}

#[derive(Debug, Clone)]
pub struct SubSplitRecord {
    pub sub_id: i32,
    pub time_sec: Option<f64>,
    pub delta_i: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct SectorRunRecord {
    pub reference_track: String,
    pub car: String,
    pub stage_slug: String,
    pub sector_index: u32,
    pub tot_sec: f64,
    pub cum_delta_sec: f64,
    pub is_complete: bool,
    pub invalidated: bool,
    pub subs: Vec<SubSplitRecord>,
}

pub struct ReferenceStore {
    conn: Connection,
}

impl ReferenceStore {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        schema::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Reference for one main-sector block (mode from config).
    pub fn resolve_reference(
        &self,
        mode: ReferenceTimeMode,
        reference_track: &str,
        car: &str,
        stage_slug: &str,
        sector_index: u32,
        sub_ids_in_order: &[i32],
    ) -> rusqlite::Result<Option<ReferenceSnapshot>> {
        match mode {
            ReferenceTimeMode::BestSector => {
                self.reference_snapshot_best_sector(
                    reference_track,
                    car,
                    stage_slug,
                    sector_index,
                    sub_ids_in_order,
                )
            }
            ReferenceTimeMode::BestSubsector => self.reference_snapshot_best_subsector(
                reference_track,
                car,
                stage_slug,
                sector_index,
                sub_ids_in_order,
            ),
            ReferenceTimeMode::BestStage => {
                let tot = self
                    .best_sector_tot_sec(reference_track, car, stage_slug, sector_index)?;
                let snap = self.reference_snapshot_best_subsector(
                    reference_track,
                    car,
                    stage_slug,
                    sector_index,
                    sub_ids_in_order,
                )?;
                Ok(snap.map(|mut s| {
                    if let Some(t) = tot {
                        s.tot_sec = t;
                    }
                    s
                }))
            }
        }
    }

    /// Fastest complete sector run; subs from that single run.
    pub fn reference_snapshot_best_sector(
        &self,
        reference_track: &str,
        car: &str,
        stage_slug: &str,
        sector_index: u32,
        sub_ids_in_order: &[i32],
    ) -> rusqlite::Result<Option<ReferenceSnapshot>> {
        let row: Option<(i64, f64)> = self
            .conn
            .query_row(
                r#"
SELECT id, tot_sec FROM reference_runs
WHERE reference_track = ?1 AND car = ?2 AND stage_slug = ?3 AND sector_index = ?4
  AND is_complete = 1 AND invalidated = 0
ORDER BY tot_sec ASC
LIMIT 1
"#,
                params![reference_track, car, stage_slug, sector_index],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();

        let Some((run_id, tot_sec)) = row else {
            return self.reference_snapshot_best_subsector_from_runs(
                reference_track,
                car,
                stage_slug,
                sector_index,
                sub_ids_in_order,
            );
        };

        let by_id = self.sub_times_for_run_id(run_id)?;
        Ok(Some(self.snapshot_from_map(
            run_id,
            sector_index,
            sub_ids_in_order,
            &by_id,
            tot_sec,
        )))
    }

    /// Per sub-gate minimum leg time in this sector; `tot_sec` = sum of those mins.
    pub fn reference_snapshot_best_subsector(
        &self,
        reference_track: &str,
        car: &str,
        stage_slug: &str,
        sector_index: u32,
        sub_ids_in_order: &[i32],
    ) -> rusqlite::Result<Option<ReferenceSnapshot>> {
        self.reference_snapshot_best_subsector_from_runs(
            reference_track,
            car,
            stage_slug,
            sector_index,
            sub_ids_in_order,
        )
    }

    fn reference_snapshot_best_subsector_from_runs(
        &self,
        reference_track: &str,
        car: &str,
        stage_slug: &str,
        sector_index: u32,
        sub_ids_in_order: &[i32],
    ) -> rusqlite::Result<Option<ReferenceSnapshot>> {
        let mut stmt = self.conn.prepare(
            r#"
SELECT srs.sub_id, MIN(srs.time_sec)
FROM sector_run_subs srs
INNER JOIN sector_runs sr ON sr.id = srs.run_id
WHERE sr.reference_track = ?1 AND sr.car = ?2 AND sr.stage_slug = ?3
  AND sr.sector_index = ?4 AND sr.is_complete = 1 AND sr.invalidated = 0
  AND srs.time_sec IS NOT NULL
GROUP BY srs.sub_id
"#,
        )?;
        let rows = stmt.query_map(
            params![reference_track, car, stage_slug, sector_index],
            |r| Ok((r.get::<_, i32>(0)?, r.get::<_, f64>(1)?)),
        )?;
        let mut by_id = std::collections::HashMap::new();
        for row in rows {
            let (id, t) = row?;
            by_id.insert(id, t);
        }
        if by_id.is_empty() {
            return Ok(None);
        }
        let sub_times_sec: Vec<f64> = sub_ids_in_order
            .iter()
            .map(|id| by_id.get(id).copied().unwrap_or(f64::NAN))
            .collect();
        let tot_sec: f64 = sub_times_sec.iter().filter(|t| t.is_finite()).sum();
        if tot_sec < 0.05 {
            return Ok(None);
        }
        Ok(Some(ReferenceSnapshot {
            run_id: 0,
            sector_index,
            sub_ids: sub_ids_in_order.to_vec(),
            sub_times_sec,
            tot_sec,
        }))
    }

    fn sub_times_for_run_id(
        &self,
        run_id: i64,
    ) -> rusqlite::Result<std::collections::HashMap<i32, f64>> {
        let mut stmt = self
            .conn
            .prepare(r#"SELECT sub_id, time_sec FROM reference_sub_splits WHERE run_id = ?1"#)?;
        let rows = stmt.query_map([run_id], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, f64>(1)?)))?;
        let mut by_id = std::collections::HashMap::new();
        for row in rows {
            let (id, t) = row?;
            by_id.insert(id, t);
        }
        Ok(by_id)
    }

    fn snapshot_from_map(
        &self,
        run_id: i64,
        sector_index: u32,
        sub_ids_in_order: &[i32],
        by_id: &std::collections::HashMap<i32, f64>,
        tot_sec: f64,
    ) -> ReferenceSnapshot {
        let sub_times_sec: Vec<f64> = sub_ids_in_order
            .iter()
            .map(|id| by_id.get(id).copied().unwrap_or(f64::NAN))
            .collect();
        ReferenceSnapshot {
            run_id,
            sector_index,
            sub_ids: sub_ids_in_order.to_vec(),
            sub_times_sec,
            tot_sec,
        }
    }

    /// Fastest complete `tot_sec` for one main sector.
    pub fn best_sector_tot_sec(
        &self,
        reference_track: &str,
        car: &str,
        stage_slug: &str,
        sector_index: u32,
    ) -> rusqlite::Result<Option<f64>> {
        let from_ref: Option<f64> = self
            .conn
            .query_row(
                r#"
SELECT tot_sec FROM reference_runs
WHERE reference_track = ?1 AND car = ?2 AND stage_slug = ?3 AND sector_index = ?4
  AND is_complete = 1 AND invalidated = 0
ORDER BY tot_sec ASC LIMIT 1
"#,
                params![reference_track, car, stage_slug, sector_index],
                |r| r.get(0),
            )
            .ok();
        if from_ref.is_some() {
            return Ok(from_ref);
        }
        Ok(self
            .conn
            .query_row(
                r#"
SELECT MIN(tot_sec) FROM sector_runs
WHERE reference_track = ?1 AND car = ?2 AND stage_slug = ?3 AND sector_index = ?4
  AND is_complete = 1 AND invalidated = 0
"#,
                params![reference_track, car, stage_slug, sector_index],
                |r| r.get(0),
            )
            .ok())
    }

    /// Composite stage PB: sum of best main-sector times (S1 + S2 + …).
    pub fn best_stage_tot_sec(
        &self,
        reference_track: &str,
        car: &str,
        stage_slug: &str,
        sector_count: u32,
    ) -> rusqlite::Result<Option<f64>> {
        let mut sum = 0.0f64;
        let mut any = false;
        for ix in 0..sector_count {
            if let Some(t) = self.best_sector_tot_sec(reference_track, car, stage_slug, ix)? {
                if t.is_finite() && t > 0.05 {
                    sum += t;
                    any = true;
                }
            }
        }
        if any {
            Ok(Some(sum))
        } else {
            Ok(None)
        }
    }

    /// Back-compat alias for [`resolve_reference`] with [`ReferenceTimeMode::BestSector`].
    pub fn reference_snapshot(
        &self,
        reference_track: &str,
        car: &str,
        stage_slug: &str,
        sector_index: u32,
        sub_ids_in_order: &[i32],
    ) -> rusqlite::Result<Option<ReferenceSnapshot>> {
        self.resolve_reference(
            ReferenceTimeMode::BestSector,
            reference_track,
            car,
            stage_slug,
            sector_index,
            sub_ids_in_order,
        )
    }

  /// Insert a sector run; returns new row id. Only complete runs may become reference (caller checks).
    pub fn insert_sector_run(&self, rec: &SectorRunRecord) -> rusqlite::Result<i64> {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        self.conn.execute(
            r#"
INSERT INTO sector_runs (
    reference_track, car, stage_slug, sector_index, tot_sec, cum_delta_sec,
    is_complete, invalidated, created_utc
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
"#,
            params![
                rec.reference_track,
                rec.car,
                rec.stage_slug,
                rec.sector_index,
                rec.tot_sec,
                rec.cum_delta_sec,
                rec.is_complete as i32,
                rec.invalidated as i32,
                now,
            ],
        )?;
        let run_id = self.conn.last_insert_rowid();
        for sub in &rec.subs {
            self.conn.execute(
                r#"INSERT INTO sector_run_subs (run_id, sub_id, time_sec, delta_i) VALUES (?1, ?2, ?3, ?4)"#,
                params![run_id, sub.sub_id, sub.time_sec, sub.delta_i],
            )?;
        }

        if rec.is_complete && !rec.invalidated {
            self.maybe_promote_reference(run_id)?;
        }
        Ok(run_id)
    }

    fn maybe_promote_reference(&self, sector_run_id: i64) -> rusqlite::Result<()> {
        let meta: (String, String, String, u32, f64, i32) = self.conn.query_row(
            r#"
SELECT reference_track, car, stage_slug, sector_index, tot_sec, is_complete
FROM sector_runs WHERE id = ?1
"#,
            [sector_run_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )?;

        if meta.5 != 1 {
            return Ok(());
        }

        let (reference_track, car, stage_slug, sector_index, tot_sec) =
            (meta.0, meta.1, meta.2, meta.3, meta.4);

        let best: Option<f64> = self
            .conn
            .query_row(
                r#"
SELECT tot_sec FROM reference_runs
WHERE reference_track = ?1 AND car = ?2 AND stage_slug = ?3 AND sector_index = ?4
  AND is_complete = 1 AND invalidated = 0
ORDER BY tot_sec ASC LIMIT 1
"#,
                params![reference_track, car, stage_slug, sector_index],
                |r| r.get(0),
            )
            .ok();

        if best.is_some_and(|b| tot_sec >= b - 1e-6) {
            return Ok(());
        }

        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        self.conn.execute(
            r#"
INSERT INTO reference_runs (
    reference_track, car, stage_slug, sector_index, tot_sec, is_complete, invalidated, created_utc
) VALUES (?1, ?2, ?3, ?4, ?5, 1, 0, ?6)
"#,
            params![reference_track, car, stage_slug, sector_index, tot_sec, now],
        )?;
        let ref_id = self.conn.last_insert_rowid();

        let mut stmt = self.conn.prepare(
            r#"SELECT sub_id, time_sec FROM sector_run_subs WHERE run_id = ?1 AND time_sec IS NOT NULL"#,
        )?;
        let rows = stmt.query_map([sector_run_id], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, f64>(1)?)))?;
        for row in rows {
            let (sub_id, time_sec) = row?;
            self.conn.execute(
                r#"INSERT INTO reference_sub_splits (run_id, sub_id, time_sec) VALUES (?1, ?2, ?3)"#,
                params![ref_id, sub_id, time_sec],
            )?;
        }
        eprintln!(
            "timing_store: new reference sector {sector_index} tot={tot_sec:.3}s ({reference_track}/{stage_slug})"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference_mode::ReferenceTimeMode;

    fn sample_run(sector_index: u32, tot: f64, subs: &[(i32, f64)]) -> SectorRunRecord {
        SectorRunRecord {
            reference_track: "hafren".into(),
            car: "car".into(),
            stage_slug: "north".into(),
            sector_index,
            tot_sec: tot,
            cum_delta_sec: 0.0,
            is_complete: true,
            invalidated: false,
            subs: subs
                .iter()
                .map(|(id, t)| SubSplitRecord {
                    sub_id: *id,
                    time_sec: Some(*t),
                    delta_i: None,
                })
                .collect(),
        }
    }

    #[test]
    fn best_stage_tot_sums_sector_bests() {
        let dir = std::env::temp_dir().join(format!("acr_ref_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("ref.sqlite");
        let store = ReferenceStore::open(&path).unwrap();
        store
            .insert_sector_run(&sample_run(0, 100.0, &[(1, 40.0), (2, 60.0)]))
            .unwrap();
        store
            .insert_sector_run(&sample_run(0, 90.0, &[(1, 30.0), (2, 60.0)]))
            .unwrap();
        store
            .insert_sector_run(&sample_run(1, 80.0, &[(3, 80.0)]))
            .unwrap();
        let stage = store
            .best_stage_tot_sec("hafren", "car", "north", 2)
            .unwrap()
            .unwrap();
        assert!((stage - 170.0).abs() < 0.01);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn best_subsector_picks_per_gate_min() {
        let dir = std::env::temp_dir().join(format!("acr_ref_sub_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("ref.sqlite");
        let store = ReferenceStore::open(&path).unwrap();
        store
            .insert_sector_run(&sample_run(0, 100.0, &[(1, 50.0), (2, 50.0)]))
            .unwrap();
        store
            .insert_sector_run(&sample_run(0, 95.0, &[(1, 40.0), (2, 55.0)]))
            .unwrap();
        let snap = store
            .resolve_reference(
                ReferenceTimeMode::BestSubsector,
                "hafren",
                "car",
                "north",
                0,
                &[1, 2],
            )
            .unwrap()
            .unwrap();
        assert!((snap.sub_times_sec[0] - 40.0).abs() < 0.01);
        assert!((snap.sub_times_sec[1] - 50.0).abs() < 0.01);
        assert!((snap.tot_sec - 90.0).abs() < 0.01);
        let _ = std::fs::remove_dir_all(dir);
    }
}
