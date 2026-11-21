//! Time-sync annotations derived from physics (offline export only).

use crate::notes::Annotation;
use crate::record::PhysicsRecord;

/// Build annotations for time synchronization from physics: first air_temp > 0, first speed_kmh > 0,
/// and each time air_temp crosses from <= 0 to > 0 (e.g. after returning to menu and re-entering).
/// Runs only during export (offline); no impact on recording performance.
///
/// Times are offsets from the first record's capture time, matching the exported `Time` column.
/// The sample rate is not constant, so the record index must not be scaled by a nominal 1/rate.
pub fn sync_annotations_from_physics(records: &[PhysicsRecord], sample_rate_hz: u32) -> Vec<Annotation> {
    let mut records = records.to_vec();
    crate::record::ensure_capture_times(&mut records, sample_rate_hz);
    let times = PhysicsRecord::motec_time_secs(&records);
    let mut out = Vec::new();
    let mut added_first_speed = false;
    for (i, r) in records.iter().enumerate() {
        let t = times.get(i).copied().unwrap_or(0.0) as f64;
        // A) Each time air_temp crosses from <= 0 to > 0
        if r.air_temp > 0.0 && (i == 0 || records[i - 1].air_temp <= 0.0) {
            out.push(Annotation {
                time_offset_sec: t,
                time_end_sec: None,
                text: format!("air_temp > 0 ({:.1} °C)", r.air_temp),
                tag: "sync_air_temp_gt_0".into(),
            });
        }
        // B) First time speed_kmh > 3 only (threshold avoids noise at standstill)
        if !added_first_speed && r.speed_kmh > 3.0 && (i == 0 || records[i - 1].speed_kmh <= 3.0) {
            added_first_speed = true;
            out.push(Annotation {
                time_offset_sec: t,
                time_end_sec: None,
                text: format!("speed_kmh > 3 ({:.1} km/h)", r.speed_kmh),
                tag: "sync_speed_gt_0".into(),
            });
        }
    }
    out.sort_by(|a, b| a.time_offset_sec.partial_cmp(&b.time_offset_sec).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::test_support::minimal_physics_record;

    fn rec(t: f64, air: f32, speed: f32) -> PhysicsRecord {
        let mut r = minimal_physics_record();
        r.capture_time_sec = t;
        r.air_temp = air;
        r.speed_kmh = speed;
        r
    }

    #[test]
    fn annotation_times_follow_capture_time_not_index() {
        // 20 samples at 100 Hz, then the rate jumps to 10 Hz: an index/333 scale would be far off.
        let mut recs = Vec::new();
        for i in 0..20 {
            recs.push(rec(10.0 + i as f64 * 0.01, 280.0, 0.0));
        }
        recs.push(rec(10.2 + 0.1, 280.0, 0.0));
        recs.push(rec(10.2 + 0.2, 280.0, 5.0)); // first speed > 3
        let ann = sync_annotations_from_physics(&recs, 333);
        let speed = ann.iter().find(|a| a.tag == "sync_speed_gt_0").unwrap();
        assert!((speed.time_offset_sec - 0.4).abs() < 1e-3, "{}", speed.time_offset_sec);
        let air = ann.iter().find(|a| a.tag == "sync_air_temp_gt_0").unwrap();
        assert_eq!(air.time_offset_sec, 0.0);
    }
}
