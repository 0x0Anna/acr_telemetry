use std::path::Path;

use rusqlite::{Connection, params};

#[derive(Debug, Clone)]
pub struct SplitRecord<'a> {
    pub track_name: &'a str,
    pub car_model: &'a str,
    pub direction: &'a str,
    pub from_sector: i32,
    pub to_sector: i32,
    pub duration_sec: f64,
    pub distance_m: f64,
}

pub fn open_or_create(path: &Path) -> Result<Connection, Box<dyn std::error::Error>> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "busy_timeout", 5000i64)?;
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS sector_splits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at_utc TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    track_name TEXT NOT NULL,
    car_model TEXT NOT NULL,
    direction TEXT NOT NULL,
    from_sector INTEGER NOT NULL,
    to_sector INTEGER NOT NULL,
    duration_sec REAL NOT NULL,
    distance_m REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS pending_splits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at_utc TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    track_name TEXT NOT NULL,
    car_model TEXT NOT NULL,
    direction TEXT NOT NULL,
    from_sector INTEGER NOT NULL,
    to_sector INTEGER NOT NULL,
    duration_sec REAL NOT NULL,
    distance_m REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sector_splits_lookup
ON sector_splits(track_name, car_model, direction, from_sector, to_sector, duration_sec);

CREATE INDEX IF NOT EXISTS idx_pending_splits_track
ON pending_splits(track_name, created_at_utc);
"#,
    )?;
    Ok(conn)
}

pub fn insert_split(conn: &Connection, rec: &SplitRecord<'_>) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute(
        r#"
INSERT INTO sector_splits (
    track_name, car_model, direction, from_sector, to_sector, duration_sec, distance_m
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
"#,
        params![
            rec.track_name,
            rec.car_model,
            rec.direction,
            rec.from_sector,
            rec.to_sector,
            rec.duration_sec,
            rec.distance_m
        ],
    )?;
    Ok(())
}

pub fn best_time(
    conn: &Connection,
    track_name: &str,
    car_model: &str,
    direction: &str,
    from_sector: i32,
    to_sector: i32,
) -> Result<Option<f64>, Box<dyn std::error::Error>> {
    let v: Option<f64> = conn.query_row(
        r#"
SELECT MIN(duration_sec)
FROM sector_splits
WHERE track_name = ?1
  AND car_model = ?2
  AND direction = ?3
  AND from_sector = ?4
  AND to_sector = ?5
"#,
        params![track_name, car_model, direction, from_sector, to_sector],
        |r| r.get(0),
    )?;
    Ok(v)
}

pub fn insert_pending_split(conn: &Connection, rec: &SplitRecord<'_>) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute(
        r#"
INSERT INTO pending_splits (
    track_name, car_model, direction, from_sector, to_sector, duration_sec, distance_m
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
"#,
        params![
            rec.track_name,
            rec.car_model,
            rec.direction,
            rec.from_sector,
            rec.to_sector,
            rec.duration_sec,
            rec.distance_m
        ],
    )?;
    Ok(())
}

pub fn promote_pending_for_track(conn: &Connection, track_name: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        r#"
INSERT INTO sector_splits (
    track_name, car_model, direction, from_sector, to_sector, duration_sec, distance_m
)
SELECT track_name, car_model, direction, from_sector, to_sector, duration_sec, distance_m
FROM pending_splits
WHERE track_name = ?1
"#,
        params![track_name],
    )?;
    let deleted = tx.execute(
        "DELETE FROM pending_splits WHERE track_name = ?1",
        params![track_name],
    )?;
    tx.commit()?;
    Ok(deleted)
}

pub fn promote_all_pending(conn: &Connection) -> Result<usize, Box<dyn std::error::Error>> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        r#"
INSERT INTO sector_splits (
    track_name, car_model, direction, from_sector, to_sector, duration_sec, distance_m
)
SELECT track_name, car_model, direction, from_sector, to_sector, duration_sec, distance_m
FROM pending_splits
"#,
        [],
    )?;
    let deleted = tx.execute("DELETE FROM pending_splits", [])?;
    tx.commit()?;
    Ok(deleted)
}
