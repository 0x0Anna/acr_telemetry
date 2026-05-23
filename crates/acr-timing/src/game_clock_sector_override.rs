//! Optional adoption of UE4SS `acr_game_clock.jsonl` sector splits after gate crossings.
//!
//! Gate detection stays on calibrated GeoJSON markers. During the run we only adopt a leg
//! when the mod has published that leg in `sectors[]` (by array index — same order as the game).
//! At Finish, [`all_stage_leg_splits_sec`] overwrites every stage leg from the final sample.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::game_clock_sync::{read_latest_sample, GameClockSample};
use crate::stage_sector_timing::StageSectorSession;
use crate::timing_sectors::GatePassMethod;

#[derive(Debug, Clone)]
pub struct GameClockSectorAdopterConfig {
    pub jsonl_path: PathBuf,
    pub max_sample_age_sec: f64,
    /// Poll JSONL at most this often (seconds).
    pub poll_interval_sec: f64,
    /// After a gate cross, keep polling up to this long before falling back to gate time.
    pub adopt_window_sec: f64,
}

#[derive(Debug, Clone)]
pub struct PendingSectorAdopt {
    pub session_si: usize,
    pub leg_ix: usize,
    pub gate_dt: f64,
    pub dt_raw: f64,
    pub leg_excess: f64,
    pub from_order: i32,
    pub to_order: i32,
    pub slug: String,
    pub label: String,
    pub pass_method: Option<GatePassMethod>,
    pub created: Instant,
    pub next_poll: Instant,
    pub deadline: Instant,
}

#[derive(Debug, Clone)]
pub struct SectorAdoptCommit {
    pub session_si: usize,
    pub leg_ix: usize,
    pub gate_dt: f64,
    pub duration_sec: f64,
    pub dt_raw: f64,
    pub leg_excess: f64,
    pub from_order: i32,
    pub to_order: i32,
    pub slug: String,
    pub label: String,
    pub pass_method: Option<GatePassMethod>,
    /// `"game_clock"`, `"game_clock_finish"`, or `"gate"`.
    pub via: &'static str,
}

pub struct GameClockSectorAdopter {
    pub cfg: GameClockSectorAdopterConfig,
    last_poll: Instant,
    cached: Option<GameClockSample>,
    jsonl_live: bool,
    pending: Vec<PendingSectorAdopt>,
}

impl GameClockSectorAdopter {
    pub fn new(cfg: GameClockSectorAdopterConfig) -> Self {
        Self {
            cfg,
            last_poll: Instant::now()
                .checked_sub(Duration::from_secs(3600))
                .unwrap_or_else(Instant::now),
            cached: None,
            jsonl_live: false,
            pending: Vec::new(),
        }
    }

    fn poll_jsonl(&mut self) {
        self.last_poll = Instant::now();
        match read_latest_sample(&self.cfg.jsonl_path, self.cfg.max_sample_age_sec) {
            Some((sample, _)) => {
                self.cached = Some(merge_game_clock_sample(self.cached.take(), sample));
                self.jsonl_live = true;
            }
            None => {
                self.jsonl_live = false;
            }
        }
    }

    pub fn poll_if_due(&mut self) {
        if self.last_poll.elapsed() < Duration::from_secs_f64(self.cfg.poll_interval_sec) {
            return;
        }
        self.poll_jsonl();
    }

    /// Always read JSONL (e.g. right after a gate cross or at Finish).
    pub fn poll_force(&mut self) {
        self.poll_jsonl();
    }

    /// JSONL exists, is fresh, and the last line parsed.
    pub fn is_live(&self) -> bool {
        self.jsonl_live && self.cached.is_some()
    }

    /// Latest cached sample (after [`poll_force`]).
    pub fn cached_sample(&self) -> Option<&GameClockSample> {
        self.cached.as_ref()
    }

    /// Adopt only when leg `leg_ix` is present in the mod array and not a duplicate of prior legs.
    pub fn split_for_leg_checked(&self, leg_ix: usize, prior_splits: &[f64]) -> Option<f64> {
        let sample = self.cached.as_ref()?;
        if !leg_ready_in_sample(sample, leg_ix) {
            return None;
        }
        let split = sector_leg_split_sec(sample, leg_ix)?;
        if is_duplicate_of_prior(split, prior_splits) {
            return None;
        }
        Some(split)
    }

    pub fn enqueue(&mut self, pending: PendingSectorAdopt) {
        self.pending.push(pending);
    }

    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    pub fn sectors_summary(&self) -> String {
        let Some(s) = self.cached.as_ref() else {
            return "(no sample)".into();
        };
        format_sectors_summary(s)
    }

    pub fn drain_commit_ready(
        &mut self,
        now: Instant,
        sessions: &[StageSectorSession],
    ) -> Vec<SectorAdoptCommit> {
        self.poll_force();
        let mut out = Vec::new();
        let poll_iv = Duration::from_secs_f64(self.cfg.poll_interval_sec);
        let mut i = 0;
        while i < self.pending.len() {
            if now < self.pending[i].next_poll {
                i += 1;
                continue;
            }
            let leg_ix = self.pending[i].leg_ix;
            let session_si = self.pending[i].session_si;
            let prior = prior_splits_from_sessions(sessions, session_si, leg_ix);
            let game_split = self.split_for_leg_checked(leg_ix, &prior);
            if let Some(split) = game_split.filter(|s| s.is_finite() && *s > 0.05) {
                let p = self.pending.remove(i);
                out.push(SectorAdoptCommit {
                    session_si: p.session_si,
                    leg_ix: p.leg_ix,
                    gate_dt: p.gate_dt,
                    duration_sec: split,
                    dt_raw: p.dt_raw,
                    leg_excess: p.leg_excess,
                    from_order: p.from_order,
                    to_order: p.to_order,
                    slug: p.slug,
                    label: p.label,
                    pass_method: p.pass_method,
                    via: "game_clock",
                });
                continue;
            }
            if now >= self.pending[i].deadline {
                let p = self.pending.remove(i);
                out.push(SectorAdoptCommit {
                    session_si: p.session_si,
                    leg_ix: p.leg_ix,
                    gate_dt: p.gate_dt,
                    duration_sec: p.gate_dt,
                    dt_raw: p.dt_raw,
                    leg_excess: p.leg_excess,
                    from_order: p.from_order,
                    to_order: p.to_order,
                    slug: p.slug,
                    label: p.label,
                    pass_method: p.pass_method,
                    via: "gate",
                });
                continue;
            }
            self.pending[i].next_poll = now + poll_iv;
            i += 1;
        }
        out
    }
}

fn is_light_sample(s: &GameClockSample) -> bool {
    s.sample_kind.as_deref() == Some("light")
}

/// Keep sector/ghost arrays from the last full scrape when the mod writes a light line.
fn merge_game_clock_sample(prev: Option<GameClockSample>, mut new: GameClockSample) -> GameClockSample {
    let Some(p) = prev else {
        return new;
    };
    if !is_light_sample(&new) {
        return new;
    }
    if new.sectors.is_empty() {
        new.sectors = p.sectors;
        if new.sectors_source.is_none() {
            new.sectors_source = p.sectors_source;
        }
        if new.sectors_debug.is_none() {
            new.sectors_debug = p.sectors_debug;
        }
        if new.next_sector_index.is_none() {
            new.next_sector_index = p.next_sector_index;
        }
    }
    if new.ghost_ref.is_none() {
        new.ghost_ref = p.ghost_ref;
    }
    new
}

fn split_from_record(rec: &crate::game_clock_sync::GameClockSectorRecord) -> Option<f64> {
    if let Some(s) = rec.split_s.filter(|v| v.is_finite() && *v > 0.05) {
        return Some(s);
    }
    None
}

fn leg_time_delta(sample: &GameClockSample, leg_ix: usize) -> Option<f64> {
    let cur = sample.sectors.get(leg_ix)?;
    let cur_t = cur.time_s.filter(|v| v.is_finite())?;
    let prev_t = if leg_ix == 0 {
        0.0
    } else {
        sample
            .sectors
            .get(leg_ix.saturating_sub(1))
            .and_then(|r| r.time_s)
            .filter(|v| v.is_finite())?
    };
    let delta = cur_t - prev_t;
    (delta > 0.05).then_some(delta)
}

pub fn prior_splits_from_sessions(
    sessions: &[StageSectorSession],
    session_si: usize,
    leg_ix: usize,
) -> Vec<f64> {
    sessions
        .get(session_si)
        .map(|sess| {
            sess.run
                .sector_secs
                .iter()
                .take(leg_ix)
                .filter_map(|t| *t)
                .collect()
        })
        .unwrap_or_default()
}

/// True when the mod has published split data for leg `leg_ix` (0 = start→S1).
pub fn leg_ready_in_sample(sample: &GameClockSample, leg_ix: usize) -> bool {
    if sample.sectors.len() > leg_ix {
        return true;
    }
    sample
        .next_sector_index
        .is_some_and(|n| n.max(0) as usize > leg_ix)
}

/// Leg split for stage leg index `leg_ix` using `sectors[leg_ix]` (game array order).
pub fn sector_leg_split_sec(sample: &GameClockSample, leg_ix: usize) -> Option<f64> {
    let rec = sample.sectors.get(leg_ix)?;
    split_from_record(rec).or_else(|| leg_time_delta(sample, leg_ix))
}

fn is_duplicate_of_prior(split: f64, prior_splits: &[f64]) -> bool {
    prior_splits
        .iter()
        .any(|p| p.is_finite() && (split - p).abs() < 0.001)
}

/// All stage leg splits from a Finish sample (`leg_count` = `sector_leg_count`).
pub fn all_stage_leg_splits_sec(sample: &GameClockSample, leg_count: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; leg_count];
    for i in 0..leg_count {
        out[i] = sector_leg_split_sec(sample, i).or_else(|| leg_time_delta(sample, i));
    }
    out
}

pub fn format_sectors_summary(sample: &GameClockSample) -> String {
    if sample.sectors.is_empty() {
        return format!(
            "sectors=0 next={:?} race_t={:?}",
            sample.next_sector_index, sample.race_time_s
        );
    }
    let parts: Vec<String> = sample
        .sectors
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "[{i}] id={:?} split={:?} cum={:?}",
                r.id,
                r.split_s.map(|v| format!("{v:.3}")),
                r.time_s.map(|v| format!("{v:.3}"))
            )
        })
        .collect();
    format!(
        "{} next={:?} race_t={:?}",
        parts.join(", "),
        sample.next_sector_index,
        sample.race_time_s.map(|v| format!("{v:.3}"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_clock_sync::GameClockSectorRecord;

    fn sample_two_sectors() -> GameClockSample {
        GameClockSample {
            race_time_s: Some(200.0),
            distance_m: None,
            race_time_valid: true,
            diff_time_s: None,
            position: None,
            phase: None,
            sectors: vec![
                GameClockSectorRecord {
                    id: Some(0),
                    time_s: Some(95.0),
                    split_s: Some(95.0),
                },
                GameClockSectorRecord {
                    id: Some(1),
                    time_s: Some(200.0),
                    split_s: Some(105.0),
                },
            ],
            sectors_source: None,
            sectors_debug: None,
            next_sector_index: Some(2),
            race_source: None,
            travel_track_id: None,
            travel_track_source: None,
            penalty_total_s: None,
            ghost_ref: None,
            game_x: None,
            game_z: None,
            t_process_ms: None,
            sample_kind: Some("full".into()),
        }
    }

    #[test]
    fn split_by_array_index_not_stale_id() {
        let sample = sample_two_sectors();
        assert!((sector_leg_split_sec(&sample, 0).unwrap() - 95.0).abs() < 1e-9);
        assert!((sector_leg_split_sec(&sample, 1).unwrap() - 105.0).abs() < 1e-9);
    }

    #[test]
    fn leg1_not_ready_with_only_one_sector() {
        let sample = GameClockSample {
            race_time_s: Some(50.0),
            distance_m: None,
            race_time_valid: true,
            diff_time_s: None,
            position: None,
            phase: None,
            sectors: vec![GameClockSectorRecord {
                id: Some(0),
                time_s: Some(45.2),
                split_s: Some(45.2),
            }],
            sectors_source: None,
            sectors_debug: None,
            next_sector_index: Some(1),
            race_source: None,
            travel_track_id: None,
            travel_track_source: None,
            penalty_total_s: None,
            ghost_ref: None,
            game_x: None,
            game_z: None,
            t_process_ms: None,
            sample_kind: None,
        };
        assert!(sector_leg_split_sec(&sample, 0).is_some());
        assert!(sector_leg_split_sec(&sample, 1).is_none());
        assert!(!leg_ready_in_sample(&sample, 1));
    }

    #[test]
    fn duplicate_prior_rejected() {
        let sample = GameClockSample {
            race_time_s: Some(200.0),
            distance_m: None,
            race_time_valid: true,
            diff_time_s: None,
            position: None,
            phase: None,
            sectors: vec![
                GameClockSectorRecord {
                    id: Some(0),
                    time_s: Some(95.0),
                    split_s: Some(95.0),
                },
                GameClockSectorRecord {
                    id: Some(1),
                    time_s: Some(190.0),
                    split_s: Some(95.0),
                },
            ],
            sectors_source: None,
            sectors_debug: None,
            next_sector_index: Some(2),
            race_source: None,
            travel_track_id: None,
            travel_track_source: None,
            penalty_total_s: None,
            ghost_ref: None,
            game_x: None,
            game_z: None,
            t_process_ms: None,
            sample_kind: Some("full".into()),
        };
        let adopter = GameClockSectorAdopter {
            cfg: GameClockSectorAdopterConfig {
                jsonl_path: PathBuf::from("x"),
                max_sample_age_sec: 1.0,
                poll_interval_sec: 1.0,
                adopt_window_sec: 2.0,
            },
            last_poll: Instant::now(),
            cached: Some(sample),
            jsonl_live: true,
            pending: Vec::new(),
        };
        assert!(adopter.split_for_leg_checked(1, &[95.0]).is_none());
    }

    #[test]
    fn all_legs_at_finish() {
        let sample = sample_two_sectors();
        let all = all_stage_leg_splits_sec(&sample, 2);
        assert_eq!(all.len(), 2);
        assert!((all[0].unwrap() - 95.0).abs() < 1e-9);
        assert!((all[1].unwrap() - 105.0).abs() < 1e-9);
    }
}
