use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use acr_recorder::export::rkyv_reader::read_rkyv;
use acr_recorder::record::PhysicsRecord;
use rusqlite::Connection;

#[derive(Clone, Debug)]
struct SessionScore {
    session_id: String,
    track: String,
    car: String,
    early_score: f64,
    correction_score: f64,
    early_traction_score: f64,
    early_brake_score: f64,
    correction_traction_score: f64,
    correction_brake_score: f64,
    early_traction_samples: usize,
    early_brake_samples: usize,
    correction_traction_samples: usize,
    correction_brake_samples: usize,
}

#[derive(Debug)]
struct WindowResult {
    traction_score: f64,
    brake_score: f64,
    combined_score: f64,
    traction_samples: usize,
    brake_samples: usize,
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  acr_grip_estimator --sqlite <telemetry.db> [--recording-id <id>] [--early-sec <sec>] [--correction-sec <sec>]");
    eprintln!("  acr_grip_estimator --rkyv <file.rkyv> [--track <name>] [--car <name>] [--early-sec <sec>] [--correction-sec <sec>]");
}

fn percentile_class(value: f64, sorted_values: &[f64]) -> u8 {
    if sorted_values.is_empty() {
        return 3;
    }
    let mut lower = 0usize;
    for (i, v) in sorted_values.iter().enumerate() {
        if *v <= value {
            lower = i + 1;
        } else {
            break;
        }
    }
    let p = lower as f64 / sorted_values.len() as f64;
    if p <= 0.2 {
        1
    } else if p <= 0.4 {
        2
    } else if p <= 0.6 {
        3
    } else if p <= 0.8 {
        4
    } else {
        5
    }
}

fn fallback_class(score: f64) -> u8 {
    if score < 0.25 {
        1
    } else if score < 0.40 {
        2
    } else if score < 0.55 {
        3
    } else if score < 0.70 {
        4
    } else {
        5
    }
}

fn mean_slip(p: &PhysicsRecord) -> f64 {
    let s = &p.wheel_slip;
    ((s.front_left.abs() + s.front_right.abs() + s.rear_left.abs() + s.rear_right.abs()) as f64) / 4.0
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        Some((values[mid - 1] + values[mid]) * 0.5)
    } else {
        Some(values[mid])
    }
}

fn estimate_window(samples: &[PhysicsRecord], sample_rate_hz: f64, start_idx: usize, duration_sec: f64, early_phase: bool) -> WindowResult {
    if samples.is_empty() {
        return WindowResult {
            traction_score: 0.5,
            brake_score: 0.5,
            combined_score: 0.5,
            traction_samples: 0,
            brake_samples: 0,
        };
    }
    let end_idx = ((start_idx as f64) + duration_sec * sample_rate_hz).round() as usize;
    let end_idx = end_idx.min(samples.len().saturating_sub(1));
    if end_idx <= start_idx + 2 {
        return WindowResult {
            traction_score: 0.5,
            brake_score: 0.5,
            combined_score: 0.5,
            traction_samples: 0,
            brake_samples: 0,
        };
    }

    let dt = 1.0 / sample_rate_hz.max(1.0);
    let mut traction_contrib = Vec::new();
    let mut brake_contrib = Vec::new();
    let mut wheelspin_events = 0usize;
    let mut traction_count = 0usize;
    let mut abs_events = 0usize;
    let mut brake_count = 0usize;

    for i in (start_idx + 1)..=end_idx {
        let prev = &samples[i - 1];
        let cur = &samples[i];
        let throttle = cur.gas as f64;
        let brake = cur.brake as f64;
        let steer = cur.steer_angle as f64;
        let speed = cur.speed_kmh as f64;

        let prev_v = prev.speed_kmh as f64 / 3.6;
        let cur_v = cur.speed_kmh as f64 / 3.6;
        let a_long = (cur_v - prev_v) / dt;
        if !a_long.is_finite() {
            continue;
        }

        let slip = mean_slip(cur);
        let is_traction_sample =
            throttle >= 0.35 && brake <= 0.10 && speed >= 10.0 && speed <= 170.0 && steer.abs() <= 0.20;
        if is_traction_sample {
            if throttle > 0.60 && slip > 0.12 && speed < 90.0 {
                wheelspin_events += 1;
            }
            let acc_term = ((a_long / throttle) / 6.0).clamp(0.0, 1.0);
            let slip_term = 1.0 - (slip / 0.25).clamp(0.0, 1.0);
            let sample_score = 0.60 * acc_term + 0.40 * slip_term;
            traction_contrib.push(sample_score);
            traction_count += 1;
        }

        let is_brake_sample =
            brake >= 0.25 && throttle <= 0.25 && speed >= 20.0 && speed <= 200.0 && steer.abs() <= 0.15;
        if is_brake_sample {
            let decel = (-a_long).max(0.0);
            let decel_term = ((decel / brake) / 9.0).clamp(0.0, 1.0);
            let slip_term = 1.0 - (slip / 0.30).clamp(0.0, 1.0);
            let sample_score = 0.70 * decel_term + 0.30 * slip_term;
            brake_contrib.push(sample_score);
            if cur.abs_in_action && speed > 40.0 {
                abs_events += 1;
            }
            brake_count += 1;
        }
    }

    let traction_score = if let Some(med) = median(&mut traction_contrib) {
        let wheelspin_ratio = wheelspin_events as f64 / traction_count.max(1) as f64;
        (med - 0.15 * wheelspin_ratio).clamp(0.0, 1.0)
    } else {
        0.5
    };
    let brake_score = if let Some(med) = median(&mut brake_contrib) {
        let abs_ratio = abs_events as f64 / brake_count.max(1) as f64;
        (med - 0.10 * abs_ratio).clamp(0.0, 1.0)
    } else {
        0.5
    };

    let (traction_w, brake_w) = if early_phase { (0.80, 0.20) } else { (0.55, 0.45) };
    let combined_score = match (traction_count > 0, brake_count > 0) {
        (true, true) => traction_w * traction_score + brake_w * brake_score,
        (true, false) => traction_score,
        (false, true) => brake_score,
        (false, false) => 0.5,
    };

    WindowResult {
        traction_score,
        brake_score,
        combined_score,
        traction_samples: traction_count,
        brake_samples: brake_count,
    }
}

fn find_launch_index(samples: &[PhysicsRecord]) -> usize {
    for (idx, p) in samples.iter().enumerate() {
        if p.speed_kmh > 5.0 {
            return idx;
        }
    }
    0
}

fn score_session(samples: &[PhysicsRecord], sample_rate_hz: f64, early_sec: f64, correction_sec: f64) -> (WindowResult, WindowResult) {
    let launch_idx = find_launch_index(samples);
    let early = estimate_window(samples, sample_rate_hz, launch_idx, early_sec, true);
    let correction = estimate_window(samples, sample_rate_hz, launch_idx, correction_sec, false);
    (early, correction)
}

fn read_scores_from_rkyv(path: &Path, track: String, car: String, early_sec: f64, correction_sec: f64) -> Result<SessionScore, Box<dyn std::error::Error>> {
    let (sample_rate, samples) = read_rkyv(path)?;
    let (early, correction) = score_session(&samples, sample_rate as f64, early_sec, correction_sec);
    Ok(SessionScore {
        session_id: path.display().to_string(),
        track,
        car,
        early_score: early.combined_score,
        correction_score: correction.combined_score,
        early_traction_score: early.traction_score,
        early_brake_score: early.brake_score,
        correction_traction_score: correction.traction_score,
        correction_brake_score: correction.brake_score,
        early_traction_samples: early.traction_samples,
        early_brake_samples: early.brake_samples,
        correction_traction_samples: correction.traction_samples,
        correction_brake_samples: correction.brake_samples,
    })
}

fn read_scores_from_sqlite(path: &Path, recording_id: Option<i64>, early_sec: f64, correction_sec: f64) -> Result<Vec<SessionScore>, Box<dyn std::error::Error>> {
    let conn = Connection::open(path)?;
    let mut sessions = Vec::new();

    let mut rid_stmt = if recording_id.is_some() {
        conn.prepare("SELECT id FROM recordings WHERE id = ?1 ORDER BY id")?
    } else {
        conn.prepare("SELECT id FROM recordings ORDER BY id")?
    };

    let mut recording_ids = Vec::new();
    if let Some(rid) = recording_id {
        let rid_rows = rid_stmt.query_map([rid], |r| r.get::<_, i64>(0))?;
        for rid_res in rid_rows {
            recording_ids.push(rid_res?);
        }
    } else {
        let rid_rows = rid_stmt.query_map([], |r| r.get::<_, i64>(0))?;
        for rid_res in rid_rows {
            recording_ids.push(rid_res?);
        }
    }

    for rid in recording_ids {
        let (track, car): (String, String) = conn.query_row(
            "SELECT COALESCE(track, 'unknown_track'), COALESCE(car_model, 'unknown_car') FROM statics WHERE recording_id = ?1",
            [rid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap_or_else(|_| ("unknown_track".to_string(), "unknown_car".to_string()));

        let mut stmt = conn.prepare(
            "SELECT gas, brake, steer_angle, speed_kmh, wheel_slip_fl, wheel_slip_fr, wheel_slip_rl, wheel_slip_rr
             FROM physics
             WHERE recording_id = ?1
             ORDER BY time_offset",
        )?;

        let iter = stmt.query_map([rid], |row| {
            Ok(PhysicsRecord {
                packet_id: 0,
                gas: row.get(0)?,
                brake: row.get(1)?,
                clutch: 0.0,
                steer_angle: row.get(2)?,
                gear: 0,
                rpm: 0,
                autoshifter_on: false,
                ignition_on: false,
                starter_engine_on: false,
                is_engine_running: true,
                speed_kmh: row.get(3)?,
                velocity: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                local_velocity: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                local_angular_vel: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                g_force: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                heading: 0.0, pitch: 0.0, roll: 0.0, final_ff: 0.0,
                wheel_slip: acr_recorder::record::WheelsRecord {
                    front_left: row.get(4)?,
                    front_right: row.get(5)?,
                    rear_left: row.get(6)?,
                    rear_right: row.get(7)?,
                },
                wheel_load: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                wheel_pressure: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                wheel_angular_speed: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                tyre_wear: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                tyre_dirty_level: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                tyre_core_temp: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                camber_rad: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                suspension_travel: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                brake_temp: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                brake_pressure: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                suspension_damage: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                slip_ratio: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                slip_angle: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                pad_life: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                disc_life: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                front_brake_compound: 0, rear_brake_compound: 0,
                tyre_temp_i: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                tyre_temp_m: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                tyre_temp_o: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                tyre_temp_extra: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                tyre_contact_point: acr_recorder::record::ContactPointRecord {
                    front_left: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                    front_right: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                    rear_left: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                    rear_right: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                },
                tyre_contact_normal: acr_recorder::record::ContactPointRecord {
                    front_left: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                    front_right: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                    rear_left: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                    rear_right: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                },
                tyre_contact_heading: acr_recorder::record::ContactPointRecord {
                    front_left: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                    front_right: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                    rear_left: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                    rear_right: acr_recorder::record::Vector3fRecord { x: 0.0, y: 0.0, z: 0.0 },
                },
                fuel: 0.0, tc: 0.0, abs: 0.0, pit_limiter_on: false, turbo_boost: 0.0,
                air_temp: 0.0, road_temp: 0.0, water_temp: 0.0,
                car_damage: acr_recorder::record::CarDamageRecord { front: 0.0, rear: 0.0, left: 0.0, right: 0.0, center: 0.0 },
                is_ai_controlled: false, brake_bias: 0.0, tc_in_action: false, abs_in_action: false,
                drs: 0, cg_height: 0.0, number_of_tyres_out: 0, kers_charge: 0.0, kers_input: 0.0,
                ride_height_front: 0.0, ride_height_rear: 0.0, ballast: 0.0, air_density: 0.0, performance_meter: 0.0,
                engine_brake: 0, ers_recovery_level: 0, ers_power_level: 0, ers_heat_charging: 0, ers_is_charging: 0,
                kers_current_kj: 0.0, drs_available: 0, drs_enabled: 0, p2p_activation: 0, p2p_status: 0, current_max_rpm: 0,
                mz: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                fz: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                my: acr_recorder::record::WheelsRecord { front_left: 0.0, front_right: 0.0, rear_left: 0.0, rear_right: 0.0 },
                kerb_vibration: 0.0, slip_vibration: 0.0, g_vibration: 0.0, abs_vibration: 0.0,
                capture_time_sec: 0.0,
            })
        })?;

        let mut samples = Vec::new();
        for p in iter {
            samples.push(p?);
        }
        if samples.len() < 500 {
            continue;
        }
        let (early, correction) = score_session(&samples, 333.0, early_sec, correction_sec);
        sessions.push(SessionScore {
            session_id: rid.to_string(),
            track,
            car,
            early_score: early.combined_score,
            correction_score: correction.combined_score,
            early_traction_score: early.traction_score,
            early_brake_score: early.brake_score,
            correction_traction_score: correction.traction_score,
            correction_brake_score: correction.brake_score,
            early_traction_samples: early.traction_samples,
            early_brake_samples: early.brake_samples,
            correction_traction_samples: correction.traction_samples,
            correction_brake_samples: correction.brake_samples,
        });
    }

    Ok(sessions)
}

fn print_results(mut sessions: Vec<SessionScore>) {
    if sessions.is_empty() {
        println!("No sessions found with enough usable samples.");
        return;
    }
    sessions.sort_by(|a, b| a.track.cmp(&b.track).then(a.car.cmp(&b.car)).then(a.session_id.cmp(&b.session_id)));

    let mut grouped: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (idx, s) in sessions.iter().enumerate() {
        grouped.entry((s.track.clone(), s.car.clone())).or_default().push(idx);
    }

    println!("session_id,track,car,early_score,early_class,early_traction_score,early_brake_score,early_traction_samples,early_brake_samples,correction_score,correction_class,correction_traction_score,correction_brake_score,correction_traction_samples,correction_brake_samples");
    for ((track, car), indices) in grouped {
        let mut early_values: Vec<f64> = indices.iter().map(|i| sessions[*i].early_score).collect();
        let mut correction_values: Vec<f64> = indices.iter().map(|i| sessions[*i].correction_score).collect();
        early_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        correction_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        let quantile_mode = indices.len() >= 8;

        for i in indices {
            let s = &sessions[i];
            let early_class = if quantile_mode {
                percentile_class(s.early_score, &early_values)
            } else {
                fallback_class(s.early_score)
            };
            let correction_class = if quantile_mode {
                percentile_class(s.correction_score, &correction_values)
            } else {
                fallback_class(s.correction_score)
            };
            println!(
                "{},{},{},{:.3},{},{:.3},{:.3},{},{},{:.3},{},{:.3},{:.3},{},{}",
                s.session_id,
                track,
                car,
                s.early_score,
                early_class,
                s.early_traction_score,
                s.early_brake_score,
                s.early_traction_samples,
                s.early_brake_samples,
                s.correction_score,
                correction_class,
                s.correction_traction_score,
                s.correction_brake_score,
                s.correction_traction_samples,
                s.correction_brake_samples
            );
        }
    }
}

fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == key).map(|w| w[1].clone())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        std::process::exit(1);
    }

    let early_sec = arg_value(&args, "--early-sec")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(10.0);
    let correction_sec = arg_value(&args, "--correction-sec")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(60.0);

    if let Some(sqlite) = arg_value(&args, "--sqlite") {
        let recording_id = arg_value(&args, "--recording-id").and_then(|v| v.parse::<i64>().ok());
        let sessions = read_scores_from_sqlite(Path::new(&sqlite), recording_id, early_sec, correction_sec)?;
        print_results(sessions);
        return Ok(());
    }

    if let Some(rkyv) = arg_value(&args, "--rkyv") {
        let track = arg_value(&args, "--track").unwrap_or_else(|| "unknown_track".to_string());
        let car = arg_value(&args, "--car").unwrap_or_else(|| "unknown_car".to_string());
        let session = read_scores_from_rkyv(&PathBuf::from(rkyv), track, car, early_sec, correction_sec)?;
        print_results(vec![session]);
        return Ok(());
    }

    print_usage();
    Ok(())
}
