//! Look up wheel-centroid position + speed at split times (seconds after movement start).

use std::env;
use std::path::PathBuf;

use acr_recorder::gis;
use acr_recorder::recording_position::{
    load_timeline, max_game_time_sec, parse_duration, pos_at_game_time, speed_at_game_time,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let physics_path = PathBuf::from(
        env::args()
            .nth(1)
            .unwrap_or_else(|| "telemetry_raw/acc_physics_1778933210.rkyv".to_string()),
    );

    let leg_s1 = parse_duration("2:07.133")?;
    let leg_s2 = parse_duration("2:17.894")?;
    let leg_s3 = parse_duration("1:54.620")?;
    let leg_finish = parse_duration("1:40.152")?;

    let (phy, tl) = load_timeline(&physics_path)?;
    let max_t = max_game_time_sec(&tl);

    eprintln!(
        "recording: {}",
        physics_path.file_name().unwrap().to_string_lossy()
    );
    eprintln!(
        "physics: {} samples | eff_hz={:.2} | movement @ rec_t={:.3}s (phy idx {})",
        tl.phy_len, tl.hz_p_eff, tl.movement_phy_sec, tl.movement_phy_idx
    );
    eprintln!("max game time after movement: {max_t:.3}s");

    let legs = [
        ("S1 (leg)", leg_s1),
        ("S2 (leg)", leg_s2),
        ("S3 (leg)", leg_s3),
        ("Finish (leg)", leg_finish),
    ];
    let mut cum = 0.0_f64;
    let mut rows: Vec<(String, f64, f64)> = Vec::new();
    for (label, leg) in legs {
        cum += leg;
        rows.push((label.to_string(), leg, cum));
    }

    println!(
        "\n{:<14} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>8} {:>6}",
        "marker", "leg_sec", "cum_sec", "game_X", "game_Y", "game_Z", "map_Z", "map_X", "spd_kmh", ""
    );
    println!("{}", "-".repeat(100));

    for (label, leg, t_game) in &rows {
        let clamped = *t_game > max_t;
        let t_use = t_game.min(max_t);
        let note = if clamped { " CLAMP" } else { "" };
        let (x, y, z) = pos_at_game_time(&phy, &tl, t_use).unwrap_or((0.0, 0.0, 0.0));
        let (map_z, map_x) = gis::game_xz_to_file(x, z);
        let spd = speed_at_game_time(&phy, &tl, t_use);
        println!(
            "{:<14} {:10.3} {:10.3} {:10.1} {:10.1} {:10.1} {:10.1} {:10.1} {:8.1}{}",
            label, leg, t_game, x, y, z, map_z, map_x, spd, note
        );
    }

    let t_last = max_t;
    let (x, y, z) = pos_at_game_time(&phy, &tl, t_last).unwrap_or((0.0, 0.0, 0.0));
    let (map_z, map_x) = gis::game_xz_to_file(x, z);
    let spd = speed_at_game_time(&phy, &tl, t_last);
    println!("\n--- last reachable game time (after movement) ---");
    println!("t_game = {t_last:.3}s");
    println!(
        "wheel center X={:.1} Y={:.1} Z={:.1} | map (Z,X)=({:.1}, {:.1}) | speed={:.1}",
        x, y, z, map_z, map_x, spd
    );

    println!("\nPosition: physics wheel centroid; time = movement_start + cum_sec (game split clock).");
    println!("eff_hz from notes wall clock (fallback 333 Hz).");
    Ok(())
}
