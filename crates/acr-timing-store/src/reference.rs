use std::path::Path;

use rusqlite::{params, Connection};

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

    /// Fastest complete reference; sub times returned in `sub_ids_in_order` (route order).
    pub fn reference_snapshot(
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
            return Ok(None);
        };

        let mut stmt = self
            .conn
            .prepare(r#"SELECT sub_id, time_sec FROM reference_sub_splits WHERE run_id = ?1"#)?;
        let rows = stmt.query_map([run_id], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, f64>(1)?)))?;
        let mut by_id = std::collections::HashMap::new();
        for row in rows {
            let (id, t) = row?;
            by_id.insert(id, t);
        }

        let mut sub_times_sec = Vec::with_capacity(sub_ids_in_order.len());
        for id in sub_ids_in_order {
            sub_times_sec.push(by_id.get(id).copied().unwrap_or(f64::NAN));
        }

        Ok(Some(ReferenceSnapshot {
            run_id,
            sector_index,
            sub_ids: sub_ids_in_order.to_vec(),
            sub_times_sec,
            tot_sec,
        }))
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
