//! Live stage-sector session: overlay strip, SQLite splits, HTML run log.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::Connection;

use crate::timing_db::{self, SplitRecord};
use crate::timing_sectors::{self, StageTimingSectors, TimingSectorRole};

pub const STAGE_TIMING_DIRECTION: &str = "stage";

#[derive(Debug, Clone)]
pub struct StageSectorRun {
    /// Timed legs: timing_start→S1, S1→S2, … (length = `sector_leg_count`).
    pub sector_secs: Vec<Option<f64>>,
    pub armed: bool,
    /// Index into `markers` for the next expected crossing.
    pub next_marker_idx: usize,
    pub anchor_clock_sec: Option<f64>,
    pub anchor_instant: Option<Instant>,
    pub completed: bool,
}

impl StageSectorRun {
    pub fn new(leg_count: usize) -> Self {
        Self {
            sector_secs: vec![None; leg_count],
            armed: false,
            next_marker_idx: 0,
            anchor_clock_sec: None,
            anchor_instant: None,
            completed: false,
        }
    }

    pub fn any_sector(&self) -> bool {
        self.sector_secs.iter().any(|t| t.is_some())
    }

    pub fn all_sectors(&self) -> bool {
        !self.sector_secs.is_empty() && self.sector_secs.iter().all(|t| t.is_some())
    }

    pub fn run_total_sec(&self) -> Option<f64> {
        let mut sum = 0.0;
        for t in &self.sector_secs {
            sum += t.as_ref()?;
        }
        Some(sum)
    }

    pub fn reset_run(&mut self) {
        let n = self.sector_secs.len();
        *self = Self::new(n);
    }

    /// Index of the leg currently being timed (`None` if not armed or run finished).
    pub fn active_leg_index(&self) -> Option<usize> {
        if !self.armed || self.completed {
            return None;
        }
        self.sector_secs.iter().position(|t| t.is_none())
    }

    /// Leg to highlight in the OSD strip (live leg, or leg 0 while waiting for timing start).
    pub fn highlight_leg_index(&self) -> Option<usize> {
        if self.completed {
            return None;
        }
        if let Some(i) = self.active_leg_index() {
            return Some(i);
        }
        if !self.armed {
            return Some(0);
        }
        None
    }

    pub fn live_leg_elapsed_sec(&self, now: Instant) -> Option<f64> {
        if self.active_leg_index().is_none() {
            return None;
        }
        self.anchor_instant
            .map(|t| now.duration_since(t).as_secs_f64())
    }
}

#[derive(Debug)]
pub struct StageSectorSession {
    pub markers: StageTimingSectors,
    pub run: StageSectorRun,
    pub html_path: Option<PathBuf>,
}

impl StageSectorSession {
    pub fn new(markers: StageTimingSectors) -> Self {
        let leg_count = markers.sector_leg_count;
        Self {
            markers,
            run: StageSectorRun::new(leg_count),
            html_path: None,
        }
    }
}

pub fn format_duration(sec: f64) -> String {
    if !sec.is_finite() || sec < 0.0 {
        return "---".to_string();
    }
    let total_ms = (sec * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{m}:{s:02}.{ms:03}")
    }
}

/// Sector strip for OSD. Use `rtss_safe = true` for RTSS (`|` separator, `(…)` highlight).
/// `live_elapsed_sec` fills the active leg while timing (otherwise `...`).
pub fn format_sector_strip(
    sector_secs: &[Option<f64>],
    rtss_safe: bool,
    highlight_leg: Option<usize>,
    live_elapsed_sec: Option<f64>,
) -> String {
    let sep = if rtss_safe { " | " } else { " · " };
    sector_secs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let s = if let Some(sec) = t {
                format_duration(*sec)
            } else if highlight_leg == Some(i) {
                live_elapsed_sec
                    .map(format_duration)
                    .unwrap_or_else(|| "...".to_string())
            } else {
                "...".to_string()
            };
            if highlight_leg == Some(i) {
                if rtss_safe {
                    format!("({s})")
                } else {
                    format!("[{s}]")
                }
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join(sep)
}

/// Third OSD line: distance to timing start or next calibrated marker.
pub fn stage_timing_osd_detail(
    session: &StageSectorSession,
    pos_x: f64,
    pos_z: f64,
) -> String {
    if session.run.completed {
        return "stage: run complete".to_string();
    }
    let next_idx = session.run.next_marker_idx.min(session.markers.markers.len().saturating_sub(1));
    let marker = &session.markers.markers[next_idx];
    let d = timing_sectors::dist_xz(pos_x, pos_z, marker.x, marker.z);
    if !session.run.armed {
        return format!(
            "stage: pass {} ({:.0}m)",
            marker.label, d
        );
    }
    format!("stage: next {} ({:.0}m)", marker.label, d)
}

pub fn compose_stage_timing_osd(status: &str, sector_strip: &str, detail: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty() {
        format!("{status}\n{sector_strip}")
    } else {
        format!("{status}\n{sector_strip}\n{detail}")
    }
}

pub fn sanitize_car_slug(car_model: &str) -> String {
    let mut out = String::new();
    let mut prev_sep = false;
    for ch in car_model.trim().chars() {
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
    if out.is_empty() {
        "unknown_car".to_string()
    } else {
        out
    }
}

pub fn new_html_path(html_dir: &Path, stage_slug: &str, car_slug: &str) -> PathBuf {
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    html_dir.join(format!("{stage_slug}_{car_slug}_{ts}.html"))
}

/// Cumulative plain-text log: `{stage_slug}_{car_slug}_runs.txt` in the HTML output dir.
pub fn runs_text_path(html_dir: &Path, stage_slug: &str, car_slug: &str) -> PathBuf {
    html_dir.join(format!("{stage_slug}_{car_slug}_runs.txt"))
}

pub fn write_or_append_text_row(
    path: &Path,
    stage_slug: &str,
    car_model: &str,
    sector_secs: &[Option<f64>],
    run_total: Option<f64>,
    run_index: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let sector_cols: Vec<String> = sector_secs
        .iter()
        .map(|t| t.map(format_duration).unwrap_or_else(|| "—".to_string()))
        .collect();
    let total_s = run_total
        .map(format_duration)
        .unwrap_or_else(|| "—".to_string());
    let line = format!(
        "{run_index}\t{now}\t{sectors}\t{total}\n",
        sectors = sector_cols.join("\t"),
        total = total_s,
    );

    if !path.exists() {
        let sector_headers: String = (0..sector_secs.len())
            .map(|i| format!("S{}", i + 1))
            .collect::<Vec<_>>()
            .join("\t");
        let header = format!(
            "# stage: {stage_slug}\n# car: {car_model}\n#\n# run\tlocal_time\t{sector_headers}\ttotal\n",
        );
        std::fs::write(path, format!("{header}{line}"))?;
        return Ok(());
    }

    std::fs::OpenOptions::new().append(true).open(path)?.write_all(line.as_bytes())?;
    Ok(())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn write_or_append_html_row(
    path: &Path,
    stage_slug: &str,
    car_model: &str,
    sector_secs: &[Option<f64>],
    run_total: Option<f64>,
    run_index: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let row_cells: Vec<String> = sector_secs
        .iter()
        .map(|t| {
            t.map(format_duration)
                .unwrap_or_else(|| "—".to_string())
        })
        .collect();
    let total_s = run_total
        .map(format_duration)
        .unwrap_or_else(|| "—".to_string());
    let row = format!(
        "<tr><td>{}</td>{cells}<td>{total}</td></tr>\n",
        run_index,
        cells = row_cells
            .iter()
            .map(|c| format!("<td>{c}</td>"))
            .collect::<String>(),
        total = html_escape(&total_s),
    );

    if !path.exists() {
        let header_cols = (0..sector_secs.len())
            .map(|i| format!("<th>S{}</th>", i + 1))
            .collect::<String>();
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{stage} — {car}</title>
<style>
body {{ font-family: system-ui, sans-serif; margin: 1.5rem; }}
table {{ border-collapse: collapse; }}
th, td {{ border: 1px solid #ccc; padding: 0.35rem 0.6rem; text-align: right; }}
th:first-child, td:first-child {{ text-align: center; }}
</style>
</head>
<body>
<h1>Stage sector times</h1>
<p><strong>Stage:</strong> {stage} &nbsp; <strong>Car:</strong> {car}</p>
<table>
<thead><tr><th>#</th>{header_cols}<th>Total</th></tr></thead>
<tbody id="runs">
{row}</tbody>
</table>
</body>
</html>
"#,
            stage = html_escape(stage_slug),
            car = html_escape(car_model),
            header_cols = header_cols,
            row = row,
        );
        std::fs::write(path, html)?;
        return Ok(());
    }

    let mut content = std::fs::read_to_string(path)?;
    if let Some(pos) = content.find("</tbody>") {
        content.insert_str(pos, &row);
    } else {
        content.push_str(&row);
    }
    std::fs::write(path, content)?;
    Ok(())
}

pub struct StageCrossOutcome {
    pub leg_index: Option<usize>,
    pub from_order: i32,
    pub to_order: i32,
    pub leg_duration_sec: Option<f64>,
    pub run_completed: bool,
    pub overlay_detail: Option<String>,
    pub pass_method: Option<timing_sectors::GatePassMethod>,
}

pub fn observe_stage_crossing(
    session: &mut StageSectorSession,
    from: (f64, f64),
    to: (f64, f64),
    radius_m: f64,
    clock_sec: f64,
    now: Instant,
) -> Option<StageCrossOutcome> {
    if session.run.completed {
        return None;
    }

    // Grid start often spawns inside the timing-start disc — arm without an outside→inside cross.
    if !session.run.armed && session.run.next_marker_idx == 0 {
        if let Some(start) = session.markers.markers.first() {
            if start.role == TimingSectorRole::TimingStart
                && timing_sectors::dist_xz(to.0, to.1, start.x, start.z) <= radius_m
            {
                session.run.armed = true;
                session.run.anchor_clock_sec = Some(clock_sec);
                session.run.anchor_instant = Some(now);
                session.run.next_marker_idx = 1;
                return Some(StageCrossOutcome {
                    leg_index: None,
                    from_order: start.order,
                    to_order: start.order,
                    leg_duration_sec: None,
                    run_completed: false,
                    overlay_detail: Some("stage timing armed (at start)".to_string()),
                    pass_method: Some(timing_sectors::GatePassMethod::RadiusDisc),
                });
            }
        }
    }

    let next_idx = session.run.next_marker_idx;
    if next_idx >= session.markers.markers.len() {
        return None;
    }
    let marker = session.markers.markers[next_idx].clone();
    let pass_method = timing_sectors::passes_timing_gate_method(
        from,
        to,
        next_idx,
        &marker,
        &session.markers.gates,
        radius_m,
    )?;
    let pass_method_str = match pass_method {
        timing_sectors::GatePassMethod::GateLine => "gate_line",
        timing_sectors::GatePassMethod::RadiusDisc => "radius_disc",
    };

    match marker.role {
        TimingSectorRole::TimingStart => {
            session.run.armed = true;
            session.run.anchor_clock_sec = Some(clock_sec);
            session.run.anchor_instant = Some(now);
            session.run.next_marker_idx = next_idx + 1;
            Some(StageCrossOutcome {
                leg_index: None,
                from_order: marker.order,
                to_order: marker.order,
                leg_duration_sec: None,
                run_completed: false,
                overlay_detail: Some(format!("stage timing armed ({pass_method_str})")),
                pass_method: Some(pass_method),
            })
        }
        TimingSectorRole::SectorBoundary | TimingSectorRole::Finish => {
            if !session.run.armed {
                return None;
            }
            let prev_order = session.markers.markers.get(next_idx.saturating_sub(1)).map(|m| m.order).unwrap_or(0);
            let dt = session
                .run
                .anchor_instant
                .map(|t| now.duration_since(t).as_secs_f64())
                .or_else(|| {
                    session.run.anchor_clock_sec.map(|st| {
                        let mut x = clock_sec - st;
                        if x < 0.0 {
                            x += 24.0 * 3600.0;
                        }
                        x
                    })
                })?;
            if dt <= 0.05 {
                session.run.next_marker_idx = next_idx + 1;
                return None;
            }
            // Leg index: 0 = start→S1, 1 = S1→S2, … (markers after timing_start).
            let leg_index = next_idx.saturating_sub(1);
            if leg_index < session.run.sector_secs.len() {
                session.run.sector_secs[leg_index] = Some(dt);
            }
            let finished = marker.role == TimingSectorRole::Finish;
            session.run.anchor_clock_sec = Some(clock_sec);
            session.run.anchor_instant = Some(now);
            session.run.next_marker_idx = next_idx + 1;
            if finished {
                session.run.completed = true;
            }
            Some(StageCrossOutcome {
                leg_index: Some(leg_index),
                from_order: prev_order,
                to_order: marker.order,
                leg_duration_sec: Some(dt),
                run_completed: finished,
                overlay_detail: Some(format!(
                    "stage S{}: {} ({pass_method_str})",
                    leg_index + 1,
                    format_duration(dt)
                )),
                pass_method: Some(pass_method),
            })
        }
    }
}

pub fn persist_stage_leg(
    conn: &Connection,
    stage_slug: &str,
    car_model: &str,
    from_order: i32,
    to_order: i32,
    duration_sec: f64,
) -> Result<f64, Box<dyn std::error::Error>> {
    let split = SplitRecord {
        track_name: stage_slug,
        car_model,
        direction: STAGE_TIMING_DIRECTION,
        from_sector: from_order,
        to_sector: to_order,
        duration_sec,
        distance_m: 0.0,
        stats: None,
    };
    let best_before = timing_db::best_time(
        conn,
        stage_slug,
        car_model,
        STAGE_TIMING_DIRECTION,
        from_order,
        to_order,
    )?;
    timing_db::insert_split(conn, &split)?;
    let delta = best_before
        .map(|b| duration_sec - b)
        .unwrap_or(0.0);
    Ok(delta)
}

pub fn flush_run_to_html(
    session: &mut StageSectorSession,
    html_dir: &Path,
    car_model: &str,
    run_counter: &mut usize,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    if !session.run.any_sector() {
        return Ok(None);
    }
    *run_counter += 1;
    let car_slug = sanitize_car_slug(car_model);
    let path = session.html_path.clone().unwrap_or_else(|| {
        new_html_path(html_dir, &session.markers.stage_slug, &car_slug)
    });
    write_or_append_html_row(
        &path,
        &session.markers.stage_slug,
        car_model,
        &session.run.sector_secs,
        session.run.run_total_sec(),
        *run_counter,
    )?;
    let text_path = runs_text_path(html_dir, &session.markers.stage_slug, &car_slug);
    write_or_append_text_row(
        &text_path,
        &session.markers.stage_slug,
        car_model,
        &session.run.sector_secs,
        session.run.run_total_sec(),
        *run_counter,
    )?;
    session.html_path = Some(path.clone());
    session.run.reset_run();
    eprintln!(
        "stage sector times appended: {} | text log: {}",
        path.display(),
        text_path.display()
    );
    Ok(Some(path))
}
