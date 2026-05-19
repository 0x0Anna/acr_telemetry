//! Pearson correlations per sector leg → `timing_factors` table.

use std::collections::HashMap;

use rusqlite::{Connection, params};

use crate::timing_db::SplitRecord;

pub const TIMING_FACTORS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS timing_factors (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    computed_at_utc TEXT NOT NULL,
    track_name TEXT NOT NULL,
    car_model TEXT NOT NULL,
    direction TEXT NOT NULL,
    from_sector INTEGER NOT NULL,
    to_sector INTEGER NOT NULL,
    target TEXT NOT NULL,
    factor TEXT NOT NULL,
    correlation REAL,
    n_samples INTEGER NOT NULL,
    n_total INTEGER NOT NULL,
    best_duration_sec REAL,
    max_duration_sec REAL,
    UNIQUE (
        track_name, car_model, direction, from_sector, to_sector, target, factor
    )
);
"#;

const METRIC_COLUMNS: &[&str] = &[
    "distance_m",
    "throttle_open_pct",
    "max_slip_angle",
    "max_slip_ratio",
    "min_slip_ratio",
    "entry_speed_kmh",
    "exit_speed_kmh",
];

const TARGETS: &[&str] = &["duration_sec", "exit_speed_kmh"];

#[derive(Debug, Clone)]
pub struct CorrelationConfig {
    pub enabled: bool,
    pub slow_pct: f64,
    pub min_samples: usize,
}

impl Default for CorrelationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            slow_pct: 10.0,
            min_samples: 4,
        }
    }
}

#[derive(Debug, Clone)]
struct Attempt {
    duration_sec: f64,
    values: HashMap<String, f64>,
}

fn pearson(xs: &[f64], ys: &[f64]) -> Option<f64> {
    let n = xs.len();
    if n < 3 || n != ys.len() {
        return None;
    }
    let mx: f64 = xs.iter().sum::<f64>() / n as f64;
    let my: f64 = ys.iter().sum::<f64>() / n as f64;
    let mut num = 0.0;
    let mut den_x = 0.0;
    let mut den_y = 0.0;
    for (x, y) in xs.iter().zip(ys.iter()) {
        let dx = x - mx;
        let dy = y - my;
        num += dx * dy;
        den_x += dx * dx;
        den_y += dy * dy;
    }
    if den_x <= 0.0 || den_y <= 0.0 {
        return None;
    }
    Some(num / (den_x.sqrt() * den_y.sqrt()))
}

fn filter_attempts(
    attempts: &[Attempt],
    slow_pct: f64,
) -> (Vec<&Attempt>, f64, f64, usize) {
    if attempts.is_empty() {
        return (Vec::new(), f64::NAN, f64::NAN, 0);
    }
    let best = attempts
        .iter()
        .map(|a| a.duration_sec)
        .fold(f64::INFINITY, f64::min);
    let limit = best * (1.0 + slow_pct / 100.0);
    let kept: Vec<&Attempt> = attempts
        .iter()
        .filter(|a| a.duration_sec <= limit + 1e-9)
        .collect();
    (kept, best, limit, attempts.len())
}

fn load_leg_attempts(
    conn: &Connection,
    track_name: &str,
    car_model: &str,
    direction: &str,
    from_sector: i32,
    to_sector: i32,
) -> Result<Vec<Attempt>, rusqlite::Error> {
    let sql = format!(
        "SELECT duration_sec, {} FROM sector_splits
         WHERE track_name = ?1 AND car_model = ?2 AND direction = ?3
           AND from_sector = ?4 AND to_sector = ?5
         ORDER BY created_at_utc",
        METRIC_COLUMNS.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![track_name, car_model, direction, from_sector, to_sector],
        |row| {
            let duration_sec: f64 = row.get(0)?;
            let mut values = HashMap::new();
            values.insert("duration_sec".to_string(), duration_sec);
            for (i, name) in METRIC_COLUMNS.iter().enumerate() {
                let v: Option<f64> = row.get(i + 1)?;
                if let Some(v) = v {
                    if v.is_finite() {
                        values.insert((*name).to_string(), v);
                    }
                }
            }
            Ok(Attempt {
                duration_sec,
                values,
            })
        },
    )?;
    rows.collect()
}

fn refresh_leg_inner(
    conn: &Connection,
    track_name: &str,
    car_model: &str,
    direction: &str,
    from_sector: i32,
    to_sector: i32,
    cfg: &CorrelationConfig,
) -> Result<usize, rusqlite::Error> {
    let attempts = load_leg_attempts(conn, track_name, car_model, direction, from_sector, to_sector)?;
    let (filtered, best, limit, n_total) = filter_attempts(&attempts, cfg.slow_pct);
    if filtered.len() < cfg.min_samples {
        return Ok(0);
    }

    let mut factor_names: std::collections::HashSet<String> = METRIC_COLUMNS
        .iter()
        .map(|s| s.to_string())
        .collect();
    for a in &filtered {
        factor_names.extend(a.values.keys().cloned());
    }
    factor_names.remove("duration_sec");
    factor_names.remove("exit_speed_kmh");

    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%fZ").to_string();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM timing_factors
         WHERE track_name = ?1 AND car_model = ?2 AND direction = ?3
           AND from_sector = ?4 AND to_sector = ?5",
        params![track_name, car_model, direction, from_sector, to_sector],
    )?;

    let mut written = 0usize;
    for target in TARGETS {
        let y: Vec<f64> = match filtered
            .iter()
            .map(|a| a.values.get(*target).copied())
            .collect::<Option<Vec<_>>>()
        {
            Some(v) => v,
            None => continue,
        };
        for factor in &factor_names {
            if factor == target {
                continue;
            }
            let xs: Vec<f64> = match filtered
                .iter()
                .map(|a| a.values.get(factor.as_str()).copied())
                .collect::<Option<Vec<_>>>()
            {
                Some(v) => v,
                None => continue,
            };
            let r = pearson(&xs, &y);
            tx.execute(
                "INSERT INTO timing_factors (
                    computed_at_utc, track_name, car_model, direction,
                    from_sector, to_sector, target, factor,
                    correlation, n_samples, n_total, best_duration_sec, max_duration_sec
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    now,
                    track_name,
                    car_model,
                    direction,
                    from_sector,
                    to_sector,
                    target,
                    factor,
                    r,
                    filtered.len() as i64,
                    n_total as i64,
                    best,
                    limit,
                ],
            )?;
            written += 1;
        }
    }
    tx.commit()?;
    Ok(written)
}

/// Recompute `timing_factors` for one sector leg.
pub fn refresh_leg(
    conn: &Connection,
    rec: &SplitRecord<'_>,
    cfg: &CorrelationConfig,
) -> Result<usize, Box<dyn std::error::Error>> {
    if !cfg.enabled {
        return Ok(0);
    }
    let n = refresh_leg_inner(
        conn,
        rec.track_name,
        rec.car_model,
        rec.direction,
        rec.from_sector,
        rec.to_sector,
        cfg,
    )?;
    Ok(n)
}

/// After promoting pending splits, refresh all legs for that track.
pub fn refresh_track(
    conn: &Connection,
    track_name: &str,
    cfg: &CorrelationConfig,
) -> Result<usize, Box<dyn std::error::Error>> {
    if !cfg.enabled {
        return Ok(0);
    }
    let mut stmt = conn.prepare(
        "SELECT DISTINCT car_model, direction, from_sector, to_sector
         FROM sector_splits WHERE track_name = ?1",
    )?;
    let legs: Vec<(String, String, i32, i32)> = stmt
        .query_map(params![track_name], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<_, _>>()?;

    let mut total = 0usize;
    for (car, dir, from, to) in legs {
        total += refresh_leg_inner(conn, track_name, &car, &dir, from, to, cfg)?;
    }
    Ok(total)
}

pub fn ensure_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(TIMING_FACTORS_DDL)
}
