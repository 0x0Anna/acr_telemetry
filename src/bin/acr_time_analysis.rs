//! Compare recording timeline (index/Hz, wall clock) vs in-game timing fields.

use std::env;
use std::path::PathBuf;

use acr_recorder::export::rkyv_reader::read_graphics_rkyv;

fn ms_to_str(ms: i32) -> String {
    if ms <= 0 {
        return "-".into();
    }
    let s = ms as f64 / 1000.0;
    let m = (s / 60.0).floor() as i32;
    let sec = s - m as f64 * 60.0;
    format!("{m}:{sec:06.3}")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(
        env::args()
            .nth(1)
            .unwrap_or_else(|| "telemetry_raw/acc_physics_1778933210.graphics.rkyv".to_string()),
    );
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .trim_end_matches(".graphics");
    let notes_path = path
        .parent()
        .unwrap()
        .join(format!("{stem}.notes.json"));
    let (hz_hdr, gfx) = read_graphics_rkyv(&path)?;
    let n = gfx.len();
    let wall_sec = std::fs::read_to_string(&notes_path)
        .ok()
        .and_then(|s| {
            let v: serde_json::Value = serde_json::from_str(&s).ok()?;
            let a = chrono::DateTime::parse_from_rfc3339(v["recording_start_utc"].as_str()?).ok()?;
            let b = chrono::DateTime::parse_from_rfc3339(v["recording_end_utc"].as_str()?).ok()?;
            Some((b - a).num_milliseconds() as f64 / 1000.0)
        });
    let eff_hz = wall_sec.map(|w| n as f64 / w);
    let t_nom = |i: usize| i as f64 / hz_hdr as f64;
    let t_wall = |i: usize| {
        eff_hz.map(|hz| i as f64 / hz).unwrap_or(t_nom(i))
    };

    println!("file: {}", path.display());
    println!(
        "samples={n} header_hz={hz_hdr} => nominal {:.3}s",
        n as f64 / hz_hdr as f64
    );
    if let Some(w) = wall_sec {
        println!("wall_clock={w:.3}s effective_hz={:.3}", n as f64 / w);
    }
    println!();
    println!(
        "{:>6} {:>8} {:>8} {:>12} {:>14} {:>10} {:>8} {:>12}",
        "idx", "t_nom", "t_wall", "dist_m", "current_ms", "time_str", "clock", "norm_pos"
    );
    for i in [0, 100, 500, 1000, 5000, 10000, 15000, 20000, 25000, n.saturating_sub(1)] {
        if i >= n {
            continue;
        }
        let g = &gfx[i];
        let ts = if g.current_time_str.is_empty() {
            g.last_time_str.as_str()
        } else {
            g.current_time_str.as_str()
        };
        println!(
            "{i:>6} {tn:>8.2} {tw:>8.2} {dist:>12.1} {ct:>14} {ts:>14} {clk:>10.1} {np:>12.4}",
            tn = t_nom(i),
            tw = t_wall(i),
            dist = g.distance_traveled,
            ct = ms_to_str(g.current_time),
            ts = if ts.len() > 14 { &ts[..14] } else { ts },
            clk = g.clock,
            np = g.normalized_car_position,
        );
    }

    // current_time stats
    let mut ct_min = i32::MAX;
    let mut ct_max = 0i32;
    let mut ct_nonzero = 0usize;
    let mut clk_nonzero = 0usize;
    for g in &gfx {
        if g.current_time > 0 {
            ct_nonzero += 1;
            ct_min = ct_min.min(g.current_time);
            ct_max = ct_max.max(g.current_time);
        }
        if g.clock > 0.01 {
            clk_nonzero += 1;
        }
    }
    println!();
    println!(
        "current_time: nonzero={ct_nonzero}/{n} range={}..{} ({:.3}s..{:.3}s)",
        if ct_min == i32::MAX { 0 } else { ct_min },
        ct_max,
        ct_min.max(0) as f64 / 1000.0,
        ct_max as f64 / 1000.0,
    );
    println!("clock: nonzero={clk_nonzero}/{n}");

    // first movement: dist > 160, speed proxy via dist delta
    let mut i_move = None;
    for i in 1..n {
        if gfx[i].distance_traveled > gfx[0].distance_traveled + 2.0 {
            i_move = Some(i);
            break;
        }
    }
    if let Some(i) = i_move {
        println!(
            "first distance increase: idx={i} t_nom={:.2}s t_wall={:.2}s dist={:.0}m current_time={}",
            t_nom(i),
            t_wall(i),
            gfx[i].distance_traveled,
            ms_to_str(gfx[i].current_time),
        );
    }

    // when current_time reaches ~479800ms (race end)
    let target_ms = 479_799;
    if let Some(i) = gfx.iter().position(|g| g.current_time >= target_ms) {
        println!(
            "current_time >= 479.799s at idx={i} t_nom={:.2}s t_wall={:.2}s dist={:.0}m",
            t_nom(i),
            t_wall(i),
            gfx[i].distance_traveled,
        );
    } else if ct_max > 0 {
        println!("current_time never reaches 479.8s (max={})", ms_to_str(ct_max));
    }

    println!();
    println!("Race splits (cum 479.799s) vs recording index/hz:");
    let splits = [(127.133, "S1"), (265.027, "S2"), (379.647, "S3"), (479.799, "Finish")];
    for (cum, label) in splits {
        let idx_nom = (cum * hz_hdr as f64).round() as usize;
        let idx_wall = eff_hz
            .map(|hz| (cum * hz).round() as usize)
            .unwrap_or(idx_nom);
        let in_range = idx_nom < n;
        println!(
            "  {label} cum={cum:.3}s => idx_nom={idx_nom} (in range={in_range}) idx_wall={idx_wall} | at idx: current_time={} dist={:.0}",
            if in_range {
                ms_to_str(gfx[idx_nom.min(n - 1)].current_time)
            } else {
                "CLAMP".into()
            },
            if in_range {
                gfx[idx_nom.min(n - 1)].distance_traveled
            } else {
                gfx[n - 1].distance_traveled
            },
        );
    }

    Ok(())
}
