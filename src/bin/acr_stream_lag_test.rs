//! Test linear time mapping between graphics (~60 Hz header) and physics (333 Hz) streams.

use std::env;
use std::path::{Path, PathBuf};

use acr_recorder::export::rkyv_reader::{read_graphics_rkyv, read_rkyv};
use acr_recorder::record::{GraphicsRecord, PhysicsRecord};

fn wheel_center_xz(p: &PhysicsRecord) -> (f64, f64) {
    let t = &p.tyre_contact_point;
    (
        (t.front_left.x + t.front_right.x + t.rear_left.x + t.rear_right.x) as f64 / 4.0,
        (t.front_left.z + t.front_right.z + t.rear_left.z + t.rear_right.z) as f64 / 4.0,
    )
}

fn physics_distance_cum(phy: &[PhysicsRecord], hz: f64) -> Vec<f64> {
    let dt = 1.0 / hz;
    let mut cum = Vec::with_capacity(phy.len());
    let mut s = 0.0;
    cum.push(0.0);
    for i in 1..phy.len() {
        let v = phy[i].speed_kmh as f64 / 3.6;
        s += v * dt;
        cum.push(s);
    }
    cum
}

fn physics_path_cum(phy: &[PhysicsRecord]) -> Vec<f64> {
    let mut cum = Vec::with_capacity(phy.len());
    let mut s = 0.0;
    let mut prev = wheel_center_xz(&phy[0]);
    cum.push(0.0);
    for p in phy.iter().skip(1) {
        let c = wheel_center_xz(p);
        let dx = c.0 - prev.0;
        let dz = c.1 - prev.1;
        s += (dx * dx + dz * dz).sqrt();
        prev = c;
        cum.push(s);
    }
    cum
}

fn wall_secs(notes_path: &Path) -> Option<f64> {
    let s = std::fs::read_to_string(notes_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let a = chrono::DateTime::parse_from_rfc3339(v["recording_start_utc"].as_str()?).ok()?;
    let b = chrono::DateTime::parse_from_rfc3339(v["recording_end_utc"].as_str()?).ok()?;
    Some((b - a).num_milliseconds() as f64 / 1000.0)
}

/// Least squares: y ≈ a*x + b
fn fit_line(xs: &[f64], ys: &[f64]) -> (f64, f64, f64) {
    let n = xs.len() as f64;
    if n < 2.0 {
        return (1.0, 0.0, 0.0);
    }
    let mx = xs.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for (&x, &y) in xs.iter().zip(ys) {
        num += (x - mx) * (y - my);
        den += (x - mx) * (x - mx);
    }
    let a = if den > 1e-12 { num / den } else { 1.0 };
    let b = my - a * mx;
    let rmse = (xs
        .iter()
        .zip(ys)
        .map(|(&x, &y)| {
            let e = y - (a * x + b);
            e * e
        })
        .sum::<f64>()
        / n)
        .sqrt();
    (a, b, rmse)
}

fn mean_pos_err(
    gfx: &[GraphicsRecord],
    phy: &[PhysicsRecord],
    k: f64,
    b: f64,
) -> f64 {
    if gfx.is_empty() {
        return 0.0;
    }
    let mut sum = 0.0;
    let mut n = 0usize;
    for (i, g) in gfx.iter().enumerate() {
        let j = (k * i as f64 + b).round() as isize;
        if j < 0 || j as usize >= phy.len() {
            continue;
        }
        let (wx, wz) = wheel_center_xz(&phy[j as usize]);
        let dx = g.car_coordinates_x as f64 - wx;
        let dz = g.car_coordinates_z as f64 - wz;
        sum += (dx * dx + dz * dz).sqrt();
        n += 1;
    }
    if n == 0 {
        f64::INFINITY
    } else {
        sum / n as f64
    }
}

fn xcorr_dist(
    dist_g: &[f32],
    dist_p: &[f64],
    max_lag_phy: isize,
) -> Vec<(isize, f64)> {
    let n = dist_g.len().min(dist_p.len());
    if n < 10 {
        return Vec::new();
    }
    let mean_g: f64 = dist_g[..n].iter().map(|&x| x as f64).sum::<f64>() / n as f64;
    let mean_p: f64 = dist_p[..n].iter().sum::<f64>() / n as f64;
    let mut out = Vec::new();
    for lag in -max_lag_phy..=max_lag_phy {
        let mut num = 0.0;
        let mut den_g = 0.0;
        let mut den_p = 0.0;
        let mut cnt = 0usize;
        for i in 0..n {
            let j = i as isize + lag;
            if j < 0 || j as usize >= dist_p.len() {
                continue;
            }
            let g = dist_g[i] as f64 - mean_g;
            let p = dist_p[j as usize] - mean_p;
            num += g * p;
            den_g += g * g;
            den_p += p * p;
            cnt += 1;
        }
        let r = if cnt > 20 && den_g > 1e-6 && den_p > 1e-6 {
            num / (den_g * den_p).sqrt()
        } else {
            0.0
        };
        out.push((lag, r));
    }
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let physics_path = PathBuf::from(
        env::args()
            .nth(1)
            .unwrap_or_else(|| "telemetry_raw/acc_physics_1778933210.rkyv".to_string()),
    );
    let stem = physics_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let gfx_path = physics_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}.graphics.rkyv"));
    let notes_path = physics_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}.notes.json"));

    let (hz_p_hdr, phy) = read_rkyv(&physics_path)?;
    let (hz_g_hdr, gfx) = read_graphics_rkyv(&gfx_path)?;
    let wall = wall_secs(&notes_path);
    let n_g = gfx.len();
    let n_p = phy.len();

    let eff_hz_g = wall.map(|w| n_g as f64 / w);
    let eff_hz_p = wall.map(|w| n_p as f64 / w);
    let ratio_samples = n_p as f64 / n_g as f64;
    let ratio_hz_hdr = hz_p_hdr as f64 / hz_g_hdr as f64;

    println!("=== stream lag / linearity test: {stem} ===\n");
    println!("physics: {n_p} samples (header {hz_p_hdr} Hz)");
    println!("graphics: {n_g} samples (header {hz_g_hdr} Hz)");
    println!("sample ratio n_p/n_g = {ratio_samples:.4} (header {ratio_hz_hdr:.4})");
    if let (Some(w), Some(hg), Some(hp)) = (wall, eff_hz_g, eff_hz_p) {
        println!("wall clock = {w:.2}s => eff_hz_g={hg:.3} eff_hz_p={hp:.3}");
        println!(
            "if same wall span: 1 gfx sample = {ratio_samples:.4} phy samples = {:.4} s wall",
            1.0 / hg
        );
    }
    println!();

    // --- 1) Match gfx[i] -> phy[j] by nearest cumulative distance (graphics dist vs phy speed integral)
    let dist_g: Vec<f32> = gfx.iter().map(|g| g.distance_traveled).collect();
    let dist_p_speed = physics_distance_cum(&phy, eff_hz_p.unwrap_or(hz_p_hdr as f64));
    let dist_p_path = physics_path_cum(&phy);

    let mut xs = Vec::new();
    let mut ys = Vec::new();
    let step = (n_g / 400).max(1);
    for i in (0..n_g).step_by(step) {
        let d = dist_g[i] as f64;
        if d < 200.0 {
            continue;
        }
        let mut best_j = 0usize;
        let mut best_e = f64::MAX;
        for (j, &pd) in dist_p_speed.iter().enumerate().step_by(8) {
            let e = (pd - d).abs();
            if e < best_e {
                best_e = e;
                best_j = j;
            }
        }
        xs.push(i as f64);
        ys.push(best_j as f64);
    }
    let (a_dist, b_dist, rmse_dist) = fit_line(&xs, &ys);
    println!("--- 1) distance matching: gfx[i] -> argmin_j |dist_gfx(i) - dist_phy_speed(j)| ---");
    println!("fit: j = {a_dist:.4} * i + {b_dist:.1}  (RMSE in phy samples: {rmse_dist:.1})");
    println!(
        "expected if linear from start: j ≈ i * {ratio_samples:.4}  => a={:.4} b≈0",
        ratio_samples
    );
    println!(
        "offset in phy samples: {b_dist:.0} (~{:.2}s @ eff physics Hz)",
        b_dist / eff_hz_p.unwrap_or(hz_p_hdr as f64)
    );
    println!();

    // --- 2) Position error vs lag (samples): j = k*i + lag
    let k0 = ratio_samples;
    let max_lag = (n_p as f64 * 0.05).round() as isize; // ±5% of phy length
    let mut best_lag = 0isize;
    let mut best_err = f64::INFINITY;
    let mut errs = Vec::new();
    for lag in -max_lag..=max_lag {
        let e = mean_pos_err(&gfx, &phy, k0, lag as f64);
        errs.push((lag, e));
        if e < best_err {
            best_err = e;
            best_lag = lag;
        }
    }
    println!("--- 2) planar position error vs constant lag (j = {k0:.4}*i + lag) ---");
    println!("best lag = {best_lag} phy samples ({:.3}s), mean err = {best_err:.2} m", 
        best_lag as f64 / eff_hz_p.unwrap_or(hz_p_hdr as f64));
    for (lag, e) in errs.iter().step_by(errs.len() / 7).take(7) {
        println!("  lag={lag:>6} => mean |Δpos| = {e:.2} m");
    }
    println!();

    // --- 3) Cross-correlation distance_traveled (gfx) vs phy integrals at phy-sample lag
    let max_xcorr_lag = (n_p as f64 * 0.03).round() as isize;
    let xcorr_speed = xcorr_dist(&dist_g, &dist_p_speed, max_xcorr_lag);
    let xcorr_path = xcorr_dist(&dist_g, &dist_p_path, max_xcorr_lag);
    let best_speed = xcorr_speed
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .map(|&(l, r)| (l, r));
    let best_path = xcorr_path
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .map(|&(l, r)| (l, r));

    println!("--- 3) cross-correlation(dist_gfx[i], dist_phy[i+lag]) ---");
    if let Some((l, r)) = best_speed {
        println!(
            "speed integral: best lag = {l} phy samples ({:.3}s), r = {r:.4}",
            l as f64 / eff_hz_p.unwrap_or(hz_p_hdr as f64)
        );
    }
    if let Some((l, r)) = best_path {
        println!(
            "wheel path length: best lag = {l} phy samples ({:.3}s), r = {r:.4}",
            l as f64 / eff_hz_p.unwrap_or(hz_p_hdr as f64)
        );
    }
    println!();

    // --- 4) Compare mapping models on middle 80% (exclude paddock + end)
    let i0 = (n_g as f64 * 0.1) as usize;
    let i1 = (n_g as f64 * 0.9) as usize;
    let models: &[(&str, f64, f64)] = &[
        ("header Hz: j = i*333/60", hz_p_hdr as f64 / hz_g_hdr as f64, 0.0),
        ("sample ratio end-anchored", ratio_samples, 0.0),
        ("fit from distance", a_dist, b_dist),
        ("best lag + ratio", k0, best_lag as f64),
    ];
    println!("--- 4) mean position error on gfx[{i0}..{i1}] ---");
    for (name, k, b) in models {
        let mut sum = 0.0;
        let mut cnt = 0usize;
        for i in i0..i1 {
            let j = (k * i as f64 + b).round() as isize;
            if j < 0 || j as usize >= phy.len() {
                continue;
            }
            let g = &gfx[i];
            let (wx, wz) = wheel_center_xz(&phy[j as usize]);
            let dx = g.car_coordinates_x as f64 - wx;
            let dz = g.car_coordinates_z as f64 - wz;
            sum += (dx * dx + dz * dz).sqrt();
            cnt += 1;
        }
        println!("  {name}: {:.2} m", sum / cnt.max(1) as f64);
    }

    // --- 5) linearity residual: j_best(i) - (a*i+b)
    let mut res = Vec::new();
    for (&i, &j) in xs.iter().zip(ys.iter()) {
        res.push(j - (a_dist * i + b_dist));
    }
    let res_mean = res.iter().sum::<f64>() / res.len().max(1) as f64;
    let res_std = (res.iter().map(|r| (r - res_mean).powi(2)).sum::<f64>() / res.len().max(1) as f64)
        .sqrt();
    println!();
    println!("--- 5) linearity of distance-matched indices (middle samples) ---");
    println!("residual j - (a*i+b): mean={res_mean:.1} std={res_std:.1} phy samples");
    println!(
        "=> {}",
        if res_std < 50.0 {
            "strong linear coupling (small residual jitter)"
        } else if res_std < 200.0 {
            "mostly linear with moderate drift/jitter"
        } else {
            "weak / non-linear — index ratio alone is unreliable"
        }
    );

    Ok(())
}
