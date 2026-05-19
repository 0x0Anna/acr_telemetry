//! HTML log for SHP subsection splits (visible and silent).

use std::path::{Path, PathBuf};

use crate::stage_sector_timing::{format_duration, sanitize_car_slug};

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn sector_label(id: i32) -> String {
    match id {
        -1 => "Start".to_string(),
        -2 => "Finish".to_string(),
        other => other.to_string(),
    }
}

fn format_delta(sec: Option<f64>) -> String {
    match sec {
        None => "—".to_string(),
        Some(d) => {
            let sign = if d >= 0.0 { "+" } else { "-" };
            format!("{sign}{}", format_duration(d.abs()))
        }
    }
}

pub fn new_html_path(html_dir: &Path, track_slug: &str, car_slug: &str) -> PathBuf {
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    html_dir.join(format!("{track_slug}_{car_slug}_subsection_{ts}.html"))
}

pub fn track_slug(track_name: &str) -> String {
    sanitize_car_slug(track_name)
}

/// Append one subsection split row (creates file + table header on first write).
pub fn append_split_row(
    path: &Path,
    track_name: &str,
    car_model: &str,
    run_index: usize,
    from_sector: i32,
    to_sector: i32,
    leg_sec: f64,
    leg_delta_sec: Option<f64>,
    cumulative_sec: f64,
    cumulative_delta_sec: Option<f64>,
    silent: bool,
    pending: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let row_class = if silent { "silent" } else { "visible" };
    let pending_note = if pending { " (pending)" } else { "" };
    let from_l = html_escape(&sector_label(from_sector));
    let to_l = html_escape(&sector_label(to_sector));
    let row = format!(
        r#"<tr class="{row_class}"><td>{run}</td><td>{from_l}</td><td>{to_l}</td><td>{leg}</td><td>{leg_d}</td><td>{cum}</td><td>{cum_d}</td><td>{kind}{pending}</td></tr>
"#,
        run = run_index,
        leg = html_escape(&format_duration(leg_sec)),
        leg_d = html_escape(&format_delta(leg_delta_sec)),
        cum = html_escape(&format_duration(cumulative_sec)),
        cum_d = html_escape(&format_delta(cumulative_delta_sec)),
        kind = if silent { "silent" } else { "visible" },
        pending = pending_note,
    );

    if !path.exists() {
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{track} — {car} (subsection)</title>
<style>
body {{ font-family: system-ui, sans-serif; margin: 1.5rem; }}
table {{ border-collapse: collapse; }}
th, td {{ border: 1px solid #ccc; padding: 0.35rem 0.6rem; text-align: right; }}
th:nth-child(1), td:nth-child(1), th:nth-child(2), td:nth-child(2), th:nth-child(3), td:nth-child(3), th:nth-child(8), td:nth-child(8) {{ text-align: center; }}
tr.silent td {{ color: #555; background: #f6f6f8; }}
tr.visible td {{ }}
</style>
</head>
<body>
<h1>Subsection splits</h1>
<p><strong>Track:</strong> {track} &nbsp; <strong>Car:</strong> {car}</p>
<p>Silent rows: pace beep vs cumulative PB; not shown on OSD.</p>
<table>
<thead><tr><th>#</th><th>From</th><th>To</th><th>Leg</th><th>Δ leg</th><th>Σ</th><th>Δ Σ</th><th>Kind</th></tr></thead>
<tbody id="splits">
{row}</tbody>
</table>
</body>
</html>
"#,
            track = html_escape(track_name),
            car = html_escape(car_model),
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
