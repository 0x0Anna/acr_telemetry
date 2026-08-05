//! Normalized blame hints when a sector split is slower than PB (sigma vs PB row + |r|).

use std::collections::HashMap;

use rusqlite::{Connection, params};

use crate::sector_leg_stats::SectorLegStatsSnapshot;
use crate::timing_correlation::CorrelationConfig;
use crate::timing_db::SplitRecord;

const METRIC_COLUMNS: &[&str] = &[
    "distance_m",
    "throttle_open_pct",
    "max_slip_angle",
    "max_slip_ratio",
    "min_slip_ratio",
    "entry_speed_kmh",
    "exit_speed_kmh",
];

#[derive(Debug, Clone)]
pub struct BlameConfig {
    pub enabled: bool,
    /// Minimum slower-than-PB delta to speak (seconds).
    pub min_delta_sec: f64,
    /// |c_f| is capped at 1.0 when |c_f| >= sigma_k (default 2σ).
    pub sigma_k: f64,
    pub max_factors: usize,
    pub min_samples: usize,
    pub slow_pct: f64,
}

impl Default for BlameConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_delta_sec: 0.05,
            sigma_k: 2.0,
            max_factors: 2,
            min_samples: 4,
            slow_pct: 10.0,
        }
    }
}

impl BlameConfig {
    pub fn from_correlation(corr: &CorrelationConfig) -> Self {
        Self {
            slow_pct: corr.slow_pct,
            min_samples: corr.min_samples,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct BlameLine {
    pub factor: String,
    pub blame_score: f64,
    pub correlation: f64,
    pub c_sigma: f64,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct BlameResult {
    pub delta_sec: f64,
    pub pb_duration_sec: f64,
    pub lines: Vec<BlameLine>,
}

impl BlameResult {
    pub fn summary_line(&self) -> String {
        if self.lines.is_empty() {
            return format!("slower +{:.2}s (no factor hint)", self.delta_sec);
        }
        let parts: Vec<String> = self
            .lines
            .iter()
            .map(|l| l.detail.clone())
            .collect();
        format!("slower +{:.2}s — {}", self.delta_sec, parts.join(", "))
    }

    pub fn voice_tokens(&self, max_factors: usize) -> Vec<String> {
        let mut out = vec!["TimingSectorSlow".to_string()];
        for line in self.lines.iter().take(max_factors) {
            if let Some(tok) = factor_voice_token(&line.factor, line.c_sigma) {
                out.push(tok.to_string());
            }
        }
        out
    }
}

fn factor_voice_token(factor: &str, c_sigma: f64) -> Option<&'static str> {
    let hurts = c_sigma > 0.0;
    Some(match factor {
        "exit_speed_kmh" => {
            if hurts {
                "TimingExitSpeedLow"
            } else {
                "TimingExitSpeedHigh"
            }
        }
        "entry_speed_kmh" => {
            if hurts {
                "TimingEntrySpeedLow"
            } else {
                "TimingEntrySpeedHigh"
            }
        }
        "throttle_open_pct" => {
            if hurts {
                "TimingThrottleHigh"
            } else {
                "TimingThrottleLow"
            }
        }
        "max_slip_angle" => {
            if hurts {
                "TimingSlipAngleHigh"
            } else {
                "TimingSlipAngleLow"
            }
        }
        "max_slip_ratio" => {
            if hurts {
                "TimingSlipHigh"
            } else {
                "TimingSlipLow"
            }
        }
        "min_slip_ratio" => {
            if hurts {
                "TimingMinSlipLow"
            } else {
                "TimingMinSlipHigh"
            }
        }
        "distance_m" => {
            if hurts {
                "TimingDistanceLong"
            } else {
                "TimingDistanceShort"
            }
        }
        _ => return None,
    })
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = v.len() / 2;
    if v.len() % 2 == 0 {
        (v[m - 1] + v[m]) / 2.0
    } else {
        v[m]
    }
}

fn robust_scale(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 1.0;
    }
    let med = median(values);
    let dev: Vec<f64> = values.iter().map(|x| (x - med).abs()).collect();
    let mad = median(&dev);
    (1.4826 * mad).max(1e-6)
}

fn snapshot_values(stats: &SectorLegStatsSnapshot) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    m.insert(
        "throttle_open_pct".to_string(),
        stats.throttle_open_pct,
    );
    m.insert("max_slip_angle".to_string(), stats.max_slip_angle as f64);
    m.insert("max_slip_ratio".to_string(), stats.max_slip_ratio as f64);
    m.insert("min_slip_ratio".to_string(), stats.min_slip_ratio as f64);
    m.insert("entry_speed_kmh".to_string(), stats.entry_speed_kmh as f64);
    m.insert("exit_speed_kmh".to_string(), stats.exit_speed_kmh as f64);
    m
}

struct Attempt {
    duration_sec: f64,
    values: HashMap<String, f64>,
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

fn filter_attempts<'a>(
    attempts: &'a [Attempt],
    slow_pct: f64,
) -> (Vec<&'a Attempt>, f64) {
    if attempts.is_empty() {
        return (Vec::new(), f64::NAN);
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
    (kept, best)
}

fn load_pb_values(
    conn: &Connection,
    track_name: &str,
    car_model: &str,
    direction: &str,
    from_sector: i32,
    to_sector: i32,
    pb_duration: f64,
) -> Result<HashMap<String, f64>, rusqlite::Error> {
    let sql = format!(
        "SELECT {} FROM sector_splits
         WHERE track_name = ?1 AND car_model = ?2 AND direction = ?3
           AND from_sector = ?4 AND to_sector = ?5
           AND duration_sec >= ?6 - 1e-6 AND duration_sec <= ?6 + 1e-6
         ORDER BY id DESC LIMIT 1",
        METRIC_COLUMNS.join(", ")
    );
    let mut values = HashMap::new();
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![
        track_name,
        car_model,
        direction,
        from_sector,
        to_sector,
        pb_duration
    ])?;
    if let Some(row) = rows.next()? {
        for (i, name) in METRIC_COLUMNS.iter().enumerate() {
            let v: Option<f64> = row.get(i)?;
            if let Some(v) = v {
                if v.is_finite() {
                    values.insert((*name).to_string(), v);
                }
            }
        }
    }
    Ok(values)
}

fn load_correlations(
    conn: &Connection,
    track_name: &str,
    car_model: &str,
    direction: &str,
    from_sector: i32,
    to_sector: i32,
) -> Result<HashMap<String, f64>, rusqlite::Error> {
    let mut out = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT factor, correlation FROM timing_factors
         WHERE track_name = ?1 AND car_model = ?2 AND direction = ?3
           AND from_sector = ?4 AND to_sector = ?5 AND target = 'duration_sec'",
    )?;
    let rows = stmt.query_map(
        params![track_name, car_model, direction, from_sector, to_sector],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<f64>>(1)?)),
    )?;
    for row in rows {
        let (factor, r) = row?;
        if let Some(r) = r {
            if r.is_finite() {
                out.insert(factor, r);
            }
        }
    }
    Ok(out)
}

fn format_factor_detail(factor: &str, x_now: f64, x_pb: f64, c_sigma: f64) -> String {
    let label = match factor {
        "exit_speed_kmh" => "exit speed",
        "entry_speed_kmh" => "entry speed",
        "throttle_open_pct" => "throttle",
        "max_slip_angle" => "slip angle",
        "max_slip_ratio" => "max slip",
        "min_slip_ratio" => "min slip",
        "distance_m" => "distance",
        _ => factor,
    };
    let delta = x_now - x_pb;
    let dir = if c_sigma > 0.0 { "vs PB hurts" } else { "vs PB ok dir" };
    if factor.contains("speed") {
        format!("{label} {:+.0} km/h {dir}", delta)
    } else if factor == "throttle_open_pct" {
        format!("{label} {:+.0}% {dir}", delta)
    } else if factor == "distance_m" {
        format!("{label} {:+.0} m {dir}", delta)
    } else {
        format!("{label} {:+.2} {dir}", delta)
    }
}

/// Analyze slower split vs PB; returns None if faster/equal or insufficient data.
pub fn analyze_slower_split(
    conn: &Connection,
    split: &SplitRecord<'_>,
    pb_duration_sec: f64,
    delta_sec: f64,
    cfg: &BlameConfig,
) -> Result<Option<BlameResult>, Box<dyn std::error::Error>> {
    if !cfg.enabled || delta_sec < cfg.min_delta_sec {
        return Ok(None);
    }

    let Some(current_stats) = split.stats else {
        return Ok(None);
    };
    let mut current = snapshot_values(&current_stats);
    current.insert("distance_m".to_string(), split.distance_m);

    let attempts = load_leg_attempts(
        conn,
        split.track_name,
        split.car_model,
        split.direction,
        split.from_sector,
        split.to_sector,
    )?;
    let (filtered, _) = filter_attempts(&attempts, cfg.slow_pct);
    if filtered.len() < cfg.min_samples {
        return Ok(None);
    }

    let pb_values = load_pb_values(
        conn,
        split.track_name,
        split.car_model,
        split.direction,
        split.from_sector,
        split.to_sector,
        pb_duration_sec,
    )?;
    if pb_values.is_empty() {
        return Ok(None);
    }

    let correlations = load_correlations(
        conn,
        split.track_name,
        split.car_model,
        split.direction,
        split.from_sector,
        split.to_sector,
    )?;
    if correlations.is_empty() {
        return Ok(None);
    }

    let mut scales: HashMap<String, f64> = HashMap::new();
    for factor in METRIC_COLUMNS {
        let hist: Vec<f64> = filtered
            .iter()
            .filter_map(|a| a.values.get(*factor).copied())
            .collect();
        if hist.len() >= 2 {
            scales.insert(factor.to_string(), robust_scale(&hist));
        }
    }

    let mut scored: Vec<BlameLine> = Vec::new();
    for (factor, r) in &correlations {
        let Some(scale) = scales.get(factor) else { continue };
        let Some(x_pb) = pb_values.get(factor) else { continue };
        let Some(x_now) = current.get(factor) else { continue };
        let d = (x_now - x_pb) / scale;
        let c_sigma = r.signum() * d;
        if !c_sigma.is_finite() || c_sigma <= 0.0 {
            continue;
        }
        let severity = (c_sigma.abs() / cfg.sigma_k).min(1.0);
        let blame_score = r.abs() * severity;
        scored.push(BlameLine {
            factor: factor.clone(),
            blame_score,
            correlation: *r,
            c_sigma,
            detail: format_factor_detail(factor, *x_now, *x_pb, c_sigma),
        });
    }

    scored.sort_by(|a, b| {
        b.blame_score
            .partial_cmp(&a.blame_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(cfg.max_factors);

    if scored.is_empty() {
        return Ok(None);
    }

    Ok(Some(BlameResult {
        delta_sec,
        pb_duration_sec,
        lines: scored,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robust_scale_positive() {
        let v = vec![10.0, 12.0, 11.0, 13.0, 10.5];
        let s = robust_scale(&v);
        assert!(s > 0.0);
    }

    #[test]
    fn blame_score_orders_by_r_and_sigma() {
        let r: f64 = 0.8;
        let c = 2.5_f64;
        let severity = (c.abs() / 2.0).min(1.0);
        let blame = r.abs() * severity;
        assert!((blame - 0.8).abs() < 1e-6);
    }
}
