//! Packet-id / wall-time analysis for a physics .rkyv vs timing log anchors.

use std::env;
use std::path::PathBuf;

use acr_recorder::export::rkyv_reader::read_rkyv;
use acr_recorder::record::PhysicsRecord;

fn wheel_xz(p: &PhysicsRecord) -> (f64, f64) {
    let t = &p.tyre_contact_point;
    (
        (t.front_left.x + t.front_right.x + t.rear_left.x + t.rear_right.x) as f64 / 4.0,
        (t.front_left.z + t.front_right.z + t.rear_left.z + t.rear_right.z) as f64 / 4.0,
    )
}

fn find_pkt(phy: &[PhysicsRecord], target: i32) -> Option<usize> {
    phy.iter()
        .position(|p| p.packet_id == target)
        .or_else(|| {
            phy.iter()
                .enumerate()
                .min_by_key(|(_, p)| (p.packet_id - target).unsigned_abs())
                .filter(|(_, p)| (p.packet_id - target).unsigned_abs() < 50)
                .map(|(i, _)| i)
        })
}

fn scan_gaps(phy: &[PhysicsRecord], lo: usize, hi: usize, _hz: f64) {
    let mut max_jump = 0i32;
    let mut max_at = 0usize;
    let mut neg = 0usize;
    for i in lo.saturating_add(1)..=hi.min(phy.len().saturating_sub(1)) {
        let dp = phy[i].packet_id - phy[i - 1].packet_id;
        if dp < 0 {
            neg += 1;
        }
        if dp > max_jump {
            max_jump = dp;
            max_at = i;
        }
    }
    println!(
        "  packet gaps: max Δpkt={max_jump} at idx {max_at}, negative steps={neg}"
    );
}

fn report_span(
    label: &str,
    phy: &[PhysicsRecord],
    pkt_a: i32,
    pkt_b: i32,
    hz: f64,
    log_dt: Option<f64>,
) {
    let Some(ia) = find_pkt(phy, pkt_a) else {
        println!("{label}: pkt {pkt_a} not found");
        return;
    };
    let Some(ib) = find_pkt(phy, pkt_b) else {
        println!("{label}: pkt {pkt_b} not found");
        return;
    };
    let (i0, i1) = if ia <= ib { (ia, ib) } else { (ib, ia) };
    let dp = (phy[i1].packet_id - phy[i0].packet_id).abs();
    let sim = dp as f64 / hz;
    let idx_dt = (i1 - i0) as f64 / hz;
    let (x0, z0) = wheel_xz(&phy[i0]);
    let (x1, z1) = wheel_xz(&phy[i1]);
    let path_m = ((x1 - x0).powi(2) + (z1 - z0).powi(2)).sqrt();
    println!("{label}:");
    println!(
        "  idx {i0}..{i1}  pkt {pa}→{pb}  Δpkt={dp}  sim={sim:.3}s  idx_dt={idx_dt:.3}s  path≈{path_m:.0}m",
        pa = phy[i0].packet_id,
        pb = phy[i1].packet_id
    );
    println!("  wheel ({x0:.1},{z0:.1}) → ({x1:.1},{z1:.1})");
    if let Some(dt) = log_dt {
        println!("  log dt={dt:.3}s  Δ(sim-log)={:+.3}s", sim - dt);
    }
    scan_gaps(phy, i0, i1, hz);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(
        env::args()
            .nth(1)
            .unwrap_or_else(|| {
                "target/release/telemetry_raw/acc_physics_1779294859.rkyv".to_string()
            }),
    );
    let (hz_hdr, phy) = read_rkyv(&path)?;
    let hz = hz_hdr as f64;
    println!("file: {}", path.display());
    println!("samples={} header_hz={hz}", phy.len());

    if phy.is_empty() {
        return Ok(());
    }
    println!("pkt range: {} .. {}", phy[0].packet_id, phy.last().unwrap().packet_id);

    // Run 2 anchors from timingproblem2.txt (pkt column)
    println!("\n=== Run 2 (HUD 7:20.978) — leg spans ===");
    report_span("S1 stage (Start→S1 stage line)", &phy, 445347, 493909, hz, Some(98.348));
    report_span(
        "S2 block (S1 geo→S2 geo)",
        &phy,
        493909,
        534237,
        hz,
        Some(121.105),
    );
    report_span(
        "S2 last sub leg CP7→S2",
        &phy,
        532677,
        534237,
        hz,
        Some(4.685),
    );
    report_span(
        "S2 stage leg (approx S1 stage→S2 stage pkt)",
        &phy,
        493909,
        532677,
        hz,
        Some(121.102),
    );
    report_span("S3 block", &phy, 534237, 576509, hz, Some(126.943));
    report_span("S3 long leg CP6→S3 (log 49.58s)", &phy, 559999, 576509, hz, Some(49.580));
    report_span("Full run (first CP1→Finish)", &phy, 464208, 608012, hz, Some(441.021));

    println!("\n=== Around Stage S2 cross (pkt 532677) ±2s sim ===");
    if let Some(ix) = find_pkt(&phy, 532677) {
        let w = (2.0 * hz) as i32;
        let lo = ix.saturating_sub(w as usize);
        let hi = (ix + w as usize).min(phy.len() - 1);
        scan_gaps(&phy, lo, hi, hz);
        for j in [ix.saturating_sub(333), ix, (ix + 333).min(phy.len() - 1)] {
            let p = &phy[j];
            let (x, z) = wheel_xz(p);
            println!(
                "  idx {j} pkt={} t_nom={:.3}s spd={:.0} xz=({x:.1},{z:.1})",
                p.packet_id,
                j as f64 / hz,
                p.speed_kmh
            );
        }
    }

    Ok(())
}
