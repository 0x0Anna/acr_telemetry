//! Plot graphics position vs time for one recording (HTML output).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use acr_recorder::export::rkyv_reader::read_graphics_rkyv;
use serde_json::json;

fn graphics_sidecar(physics: &Path) -> PathBuf {
    let stem = physics.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    physics
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}.graphics.rkyv"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let physics = PathBuf::from(
        env::args()
            .nth(1)
            .unwrap_or_else(|| "telemetry_raw/acc_physics_1778921308.rkyv".to_string()),
    );
    let out = env::args().nth(2).map(PathBuf::from).unwrap_or_else(|| {
        physics
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!(
                "{}_plot.html",
                physics.file_stem().and_then(|s| s.to_str()).unwrap_or("recording")
            ))
    });

    let gfx_path = graphics_sidecar(&physics);
    let (hz, gfx) = read_graphics_rkyv(&gfx_path)?;
    let hz_f = hz as f64;
    if gfx.is_empty() {
        return Err("no graphics records".into());
    }

    let t: Vec<f64> = (0..gfx.len()).map(|i| i as f64 / hz_f).collect();
    let x: Vec<f64> = gfx.iter().map(|r| r.car_coordinates_x as f64).collect();
    let y: Vec<f64> = gfx.iter().map(|r| r.car_coordinates_y as f64).collect();
    let z: Vec<f64> = gfx.iter().map(|r| r.car_coordinates_z as f64).collect();
    let dist: Vec<f64> = gfx.iter().map(|r| r.distance_traveled as f64).collect();

    let timing_marks = load_timing_marks(&physics);

    let html = render_html(&physics, &gfx_path, hz, &t, &x, &y, &z, &dist, &timing_marks);
    fs::write(&out, html)?;
    eprintln!("wrote {}", out.display());
    Ok(())
}

#[derive(Clone)]
struct TimingMark {
    label: String,
    t_sec: f64,
    x: f64,
    y: f64,
    z: f64,
}

fn load_timing_marks(physics: &Path) -> Vec<TimingMark> {
    let slug_path = Path::new("timing/timing_sectors.geojson");
    let raw = match fs::read_to_string(slug_path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let root: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let src = physics
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if root
        .get("properties")
        .and_then(|p| p.get("source_recording"))
        .and_then(|v| v.as_str())
        != Some(src)
    {
        eprintln!("note: timing_sectors.geojson is for another recording");
    }
    let t0 = root
        .get("properties")
        .and_then(|p| p.get("timing_start_offset_sec"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let mut marks = Vec::new();
    if let Some(features) = root.get("features").and_then(|v| v.as_array()) {
        for f in features {
            let p = &f["properties"];
            let label = p
                .get("marker_label")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let from_t0 = p
                .get("time_from_timing_start_sec")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let t_sec = p
                .get("time_offset_sec")
                .and_then(|v| v.as_f64())
                .unwrap_or(t0 + from_t0);
            let gx = p.get("game_x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let gy = p
                .get("game_y")
                .and_then(|v| v.as_f64())
                .or_else(|| {
                    f.get("geometry")
                        .and_then(|g| g.get("coordinates"))
                        .and_then(|c| c.get(1))
                        .and_then(|v| v.as_f64())
                })
                .unwrap_or(0.0);
            let gz = p.get("game_z").and_then(|v| v.as_f64()).unwrap_or(0.0);
            marks.push(TimingMark {
                label: label.to_string(),
                t_sec,
                x: gx,
                y: gy,
                z: gz,
            });
        }
    }
    marks.sort_by(|a, b| a.t_sec.partial_cmp(&b.t_sec).unwrap_or(std::cmp::Ordering::Equal));
    let _ = t0;
    marks
}

fn render_html(
    physics: &Path,
    gfx_path: &Path,
    hz: u32,
    t: &[f64],
    x: &[f64],
    y: &[f64],
    z: &[f64],
    dist: &[f64],
    marks: &[TimingMark],
) -> String {
    let data = json!({
        "t": t,
        "x": x,
        "y": y,
        "z": z,
        "dist": dist,
        "marks": marks.iter().map(|m| json!({
            "label": m.label,
            "t": m.t_sec,
            "x": m.x,
            "y": m.y,
            "z": m.z,
        })).collect::<Vec<_>>(),
    });
    let data_js = serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_string());
    let title = physics
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<title>{title} — position vs time</title>
<script src="https://cdn.plot.ly/plotly-2.35.2.min.js"></script>
<style>
body {{ font-family: system-ui, sans-serif; margin: 12px; background: #111; color: #eee; }}
h1 {{ font-size: 1.1rem; }}
.meta {{ color: #aaa; font-size: 0.85rem; margin-bottom: 8px; }}
#xz, #tx, #ty, #tz, #td {{ width: 100%; height: 420px; margin-bottom: 8px; }}
</style>
</head>
<body>
<h1>{title}</h1>
<p class="meta">Graphics: {gfx} @ {hz} Hz — car_coordinates_x/y/z (game world). Vertical lines = timing_sectors.geojson offsets.</p>
<div id="xz"></div>
<div id="tx"></div>
<div id="ty"></div>
<div id="tz"></div>
<div id="td"></div>
<script>
const DATA = {data_js};
const marks = DATA.marks || [];
function vlines(fig) {{
  return marks.map(m => ({{
    type: 'line', xref: 'x', yref: 'paper', x0: m.t, x1: m.t, y0: 0, y1: 1,
    line: {{ color: 'rgba(255,200,80,0.85)', width: 1, dash: 'dot' }},
    name: m.label
  }}));
}}
function markAnnotations() {{
  return marks.map(m => ({{
    x: m.t, y: 1.02, xref: 'x', yref: 'paper', text: m.label + '<br>' + m.t.toFixed(1) + 's',
    showarrow: false, font: {{ size: 10, color: '#fc8' }}
  }}));
}}
Plotly.newPlot('xz', [
  {{ x: DATA.x, y: DATA.z, mode: 'lines', line: {{ color: '#6af', width: 1 }}, name: 'XZ path',
     hovertemplate: 't=%{{customdata:.1f}}s<br>X=%{{x:.1f}}<br>Z=%{{y:.1f}}<extra></extra>',
     customdata: DATA.t }},
  ...marks.map(m => ({{ x: [m.x], y: [m.z], mode: 'markers+text', text: [m.label],
    textposition: 'top center', marker: {{ size: 10, color: '#fc8' }}, name: m.label }}))
], {{ title: 'Plan XZ (color = time along line)', paper_bgcolor: '#111', plot_bgcolor: '#1a1a1a',
  font: {{ color: '#ddd' }}, xaxis: {{ title: 'X' }}, yaxis: {{ title: 'Z' }}, showlegend: true }},
  {{ responsive: true }});
Plotly.newPlot('tx', [
  {{ x: DATA.t, y: DATA.x, mode: 'lines', line: {{ color: '#8f8' }}, name: 'X' }},
  ...vlines()
], {{ title: 'X vs time_offset', shapes: [], annotations: markAnnotations(),
  paper_bgcolor: '#111', plot_bgcolor: '#1a1a1a', font: {{ color: '#ddd' }},
  xaxis: {{ title: 'time (s) from graphics frame 0' }}, yaxis: {{ title: 'X' }} }},
  {{ responsive: true }});
Plotly.newPlot('ty', [
  {{ x: DATA.t, y: DATA.y, mode: 'lines', line: {{ color: '#f88' }}, name: 'Y (height)' }},
  ...vlines()
], {{ title: 'Y vs time_offset', annotations: markAnnotations(),
  paper_bgcolor: '#111', plot_bgcolor: '#1a1a1a', font: {{ color: '#ddd' }},
  xaxis: {{ title: 'time (s)' }}, yaxis: {{ title: 'Y' }} }},
  {{ responsive: true }});
Plotly.newPlot('tz', [
  {{ x: DATA.t, y: DATA.z, mode: 'lines', line: {{ color: '#88f' }}, name: 'Z' }},
  ...vlines()
], {{ title: 'Z vs time_offset', annotations: markAnnotations(),
  paper_bgcolor: '#111', plot_bgcolor: '#1a1a1a', font: {{ color: '#ddd' }},
  xaxis: {{ title: 'time (s)' }}, yaxis: {{ title: 'Z' }} }},
  {{ responsive: true }});
Plotly.newPlot('td', [
  {{ x: DATA.t, y: DATA.dist, mode: 'lines', line: {{ color: '#ddd' }}, name: 'distance_traveled' }},
  ...vlines()
], {{ title: 'distance_traveled vs time', annotations: markAnnotations(),
  paper_bgcolor: '#111', plot_bgcolor: '#1a1a1a', font: {{ color: '#ddd' }},
  xaxis: {{ title: 'time (s)' }}, yaxis: {{ title: 'm' }} }},
  {{ responsive: true }});
</script>
</body>
</html>
"#,
        title = html_escape(title),
        gfx = html_escape(&gfx_path.display().to_string()),
        hz = hz,
        data_js = data_js,
    )
}

fn html_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            _ => c.to_string(),
        })
        .collect()
}
