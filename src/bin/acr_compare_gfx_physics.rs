//! Compare graphics car_coordinates with physics tyre-contact centroid over time.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use acr_recorder::export::rkyv_reader::{read_graphics_rkyv, read_rkyv};
use acr_recorder::record::{GraphicsRecord, PhysicsRecord};

/// Map graphics sample index → physics index (streams share start but may differ in length).
fn physics_for_graphics_index<'a>(
    phy: &'a [PhysicsRecord],
    gfx_i: usize,
    gfx_len: usize,
) -> &'a PhysicsRecord {
    if gfx_len <= 1 || phy.is_empty() {
        return &phy[0];
    }
    let idx_f = gfx_i as f64 * (phy.len() - 1) as f64 / (gfx_len - 1) as f64;
    let i = idx_f.round() as usize;
    &phy[i.min(phy.len() - 1)]
}

fn wheel_center_xz(p: &PhysicsRecord) -> (f64, f64, f64) {
    let t = &p.tyre_contact_point;
    let x = (t.front_left.x + t.front_right.x + t.rear_left.x + t.rear_right.x) as f64 / 4.0;
    let y = (t.front_left.y + t.front_right.y + t.rear_left.y + t.rear_right.y) as f64 / 4.0;
    let z = (t.front_left.z + t.front_right.z + t.rear_left.z + t.rear_right.z) as f64 / 4.0;
    (x, y, z)
}

fn player_graphics_pos(g: &GraphicsRecord) -> (f64, f64, f64) {
    (
        g.car_coordinates_x as f64,
        g.car_coordinates_y as f64,
        g.car_coordinates_z as f64,
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let physics_path = PathBuf::from(
        env::args()
            .nth(1)
            .unwrap_or_else(|| "telemetry_raw/acc_physics_1778933210.rkyv".to_string()),
    );
    let out = env::args().nth(2).map(PathBuf::from).unwrap_or_else(|| {
        physics_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!(
                "{}_gfx_physics_compare.html",
                physics_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("rec")
            ))
    });

    let gfx_path = physics_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            "{}.graphics.rkyv",
            physics_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        ));

    let (_hz_p, phy) = read_rkyv(&physics_path)?;
    let (hz_g, gfx) = read_graphics_rkyv(&gfx_path)?;
    let hz_g = hz_g as f64;

    let mut t = Vec::new();
    let mut gx = Vec::new();
    let mut gz = Vec::new();
    let mut wx = Vec::new();
    let mut wz = Vec::new();
    let mut dx = Vec::new();
    let mut dz = Vec::new();
    let mut dist = Vec::new();

    for (i, g) in gfx.iter().enumerate() {
        let (px, _py, pz) = player_graphics_pos(g);
        let time = i as f64 / hz_g;
        let p = physics_for_graphics_index(&phy, i, gfx.len());
        let (cx, _cy, cz) = wheel_center_xz(p);
        let ddx = px - cx;
        let ddz = pz - cz;
        t.push(time);
        gx.push(px);
        gz.push(pz);
        wx.push(cx);
        wz.push(cz);
        dx.push(ddx);
        dz.push(ddz);
        dist.push((ddx * ddx + ddz * ddz).sqrt());
    }

    let n = t.len();
    let mean_d = dist.iter().sum::<f64>() / n.max(1) as f64;
    let on_track: Vec<f64> = dist.iter().copied().filter(|&d| d < 5.0).collect();
    let mean_on_track = on_track.iter().sum::<f64>() / on_track.len().max(1) as f64;
    let mean_dx = dx.iter().sum::<f64>() / n.max(1) as f64;
    let mean_dz = dz.iter().sum::<f64>() / n.max(1) as f64;
    let residual: Vec<f64> = dx
        .iter()
        .zip(&dz)
        .map(|(&x, &z)| {
            let rx = x - mean_dx;
            let rz = z - mean_dz;
            (rx * rx + rz * rz).sqrt()
        })
        .collect();
    let mean_residual = residual.iter().sum::<f64>() / n.max(1) as f64;
    let max_d = dist.iter().cloned().fold(0.0_f64, f64::max);
    let max_i = dist
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i)
        .unwrap_or(0);

    eprintln!(
        "samples={n} | mean |Δ|={mean_d:.2} m (|Δ|<5m: {mean_on_track:.2} m, n={}) | mean offset ΔX={mean_dx:.2} ΔZ={mean_dz:.2} | residual σ≈{mean_residual:.2} m | max={max_d:.1} m @ t={:.1}s",
        on_track.len(),
        t[max_i]
    );
    for &i in &[0, 100, 1000, 5000] {
        if i >= n {
            continue;
        }
        eprintln!(
            "  t={:.2}s gfx=({:.1},{:.1}) wheel=({:.1},{:.1}) d={:.1}m",
            t[i], gx[i], gz[i], wx[i], wz[i], dist[i]
        );
    }

    let data = serde_json::json!({
        "t": t,
        "gx": gx, "gz": gz,
        "wx": wx, "wz": wz,
        "dx": dx, "dz": dz, "dist": dist,
    });
    let data_js = serde_json::to_string(&data)?;
    let title = physics_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");

    let phy_len = phy.len();
    let gfx_len = gfx.len();
    let html = format!(
        r##"<!DOCTYPE html>
<html><head><meta charset="utf-8"/>
<title>{title} — graphics vs physics wheel center</title>
<script src="https://cdn.plot.ly/plotly-2.35.2.min.js"></script>
<style>body{{font-family:system-ui;background:#111;color:#eee;margin:12px}}
h1{{font-size:1.1rem}} .meta{{color:#aaa;font-size:0.85rem}}</style></head>
<body>
<h1>{title}</h1>
<p class="meta">Graphics <code>car_coordinates</code> vs physics mean <code>tyre_contact_point</code> (4 wheels). Physics index aligned proportionally to graphics length ({phy_len} vs {gfx_len} samples).</p>
<p class="meta">Mean |Δ| XZ: <b>{mean_d:.2} m</b> (samples with |Δ|&lt;5m: <b>{mean_on_track:.2} m</b>) | constant offset ΔX={mean_dx:.2} ΔZ={mean_dz:.2} m | spread after offset: <b>{mean_residual:.2} m</b> | max: <b>{max_d:.1} m</b></p>
<div id="xz" style="width:100%;height:440px"></div>
<div id="err" style="width:100%;height:320px"></div>
<div id="comp" style="width:100%;height:320px"></div>
<script>
const D = {data_js};
Plotly.newPlot('xz', [
  {{x:D.gx,y:D.gz,mode:'lines',name:'graphics XZ',line:{{color:'#6af',width:1}}}},
  {{x:D.wx,y:D.wz,mode:'lines',name:'physics wheel center XZ',line:{{color:'#f84',width:1}}}}
], {{title:'Plan view',paper_bgcolor:'#111',plot_bgcolor:'#1a1a1a',font:{{color:'#ddd'}},
  xaxis:{{title:'X'}},yaxis:{{title:'Z'}}}}, {{responsive:true}});
Plotly.newPlot('err', [
  {{x:D.t,y:D.dist,mode:'lines',name:'|Δ| XZ',line:{{color:'#ddd'}}}}
], {{title:'Distance graphics − wheel center (m)',paper_bgcolor:'#111',plot_bgcolor:'#1a1a1a',
  font:{{color:'#ddd'}},xaxis:{{title:'t (s)'}},yaxis:{{title:'m'}}}}, {{responsive:true}});
Plotly.newPlot('comp', [
  {{x:D.t,y:D.dx,mode:'lines',name:'ΔX',line:{{color:'#8f8'}}}},
  {{x:D.t,y:D.dz,mode:'lines',name:'ΔZ',line:{{color:'#88f'}}}}
], {{title:'ΔX / ΔZ vs time',paper_bgcolor:'#111',plot_bgcolor:'#1a1a1a',
  font:{{color:'#ddd'}},xaxis:{{title:'t (s)'}},yaxis:{{title:'m'}}}}, {{responsive:true}});
</script></body></html>"##,
        title = title,
        phy_len = phy_len,
        gfx_len = gfx_len,
        mean_d = mean_d,
        mean_on_track = mean_on_track,
        mean_dx = mean_dx,
        mean_dz = mean_dz,
        mean_residual = mean_residual,
        max_d = max_d,
        data_js = data_js,
    );
    fs::write(&out, html)?;
    eprintln!("wrote {}", out.display());
    Ok(())
}
