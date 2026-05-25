//! Optional adoption of UE4SS `acr_game_clock.jsonl` sector splits after gate crossings.
//!
//! Gate detection stays on calibrated GeoJSON markers. During the run we only adopt a leg
//! when the mod has published that leg in `sectors[]` (by array index — same order as the game).
//! At Finish, [`all_stage_leg_splits_sec`] overwrites every stage leg from the final sample.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::game_clock_sync::{merge_game_clock_sample, read_latest_sample, GameClockSample};
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

#[derive(Debug, Clone)]
pub struct PendingFinishAdopt {
    pub session_si: usize,
    pub label: String,
    pub slug: String,
    pub leg_count: usize,
    pub next_poll: Instant,
    pub deadline: Instant,
}

/// One leg updated from UE4SS after Finish (for logging / DB).
#[derive(Debug, Clone)]
pub struct FinishLegOverride {
    pub session_si: usize,
    pub leg_ix: usize,
    pub prev_sec: f64,
    pub ue4ss_sec: f64,
}

pub struct GameClockSectorAdopter {
    pub cfg: GameClockSectorAdopterConfig,
    last_poll: Instant,
    cached: Option<GameClockSample>,
    jsonl_live: bool,
    pending: Vec<PendingSectorAdopt>,
    /// Poll JSONL after Finish until all sector legs have UE4SS times (S4 often lags).
    finish_pending: Option<PendingFinishAdopt>,
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
            finish_pending: None,
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

    pub fn cached_sample_owned(&self) -> Option<GameClockSample> {
        self.cached.clone()
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

    /// After Finish: keep polling until every main-sector leg has a UE4SS split (esp. S4).
    pub fn enqueue_finish_retry(
        &mut self,
        session_si: usize,
        label: String,
        slug: String,
        leg_count: usize,
    ) {
        let poll_iv = Duration::from_secs_f64(self.cfg.poll_interval_sec);
        let window = Duration::from_secs_f64(self.cfg.adopt_window_sec.max(3.0));
        let now = Instant::now();
        self.finish_pending = Some(PendingFinishAdopt {
            session_si,
            label,
            slug,
            leg_count,
            next_poll: now + poll_iv,
            deadline: now + window,
        });
    }

    pub fn has_finish_pending(&self) -> bool {
        self.finish_pending.is_some()
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

fn split_from_record(rec: &crate::game_clock_sync::GameClockSectorRecord) -> Option<f64> {
    if let Some(s) = rec.split_s.filter(|v| v.is_finite() && *v > 0.05) {
        return Some(s);
    }
    None
}

fn record_adoptable(rec: &crate::game_clock_sync::GameClockSectorRecord) -> bool {
    split_from_record(rec).is_some()
        || rec
            .time_s
            .filter(|t| t.is_finite() && *t > 0.05)
            .is_some()
}

fn sector_id_matches_leg(rec: &crate::game_clock_sync::GameClockSectorRecord, leg_ix: usize) -> bool {
    match rec.id {
        None => true,
        Some(0) if leg_ix == 0 => true,
        Some(id) => id == leg_ix as i32 + 1,
    }
}

/// Map stage leg index (0 = Start→S1) to the game's `sectors[]` row.
///
/// ACR often has `id:0` / `split_s:0` at array index 0 (start line); timed leg *i* uses `id: i+1`.
fn sector_record_for_leg<'a>(
    sample: &'a GameClockSample,
    leg_ix: usize,
) -> Option<&'a crate::game_clock_sync::GameClockSectorRecord> {
    if let Some(rec) = sample.sectors.get(leg_ix) {
        if record_adoptable(rec) && sector_id_matches_leg(rec, leg_ix) {
            return Some(rec);
        }
    }
    if sample
        .sectors
        .first()
        .is_some_and(|r| !record_adoptable(r) && r.id == Some(0))
    {
        if let Some(rec) = sample.sectors.get(leg_ix + 1) {
            if record_adoptable(rec) {
                return Some(rec);
            }
        }
    }
    let game_id = leg_ix as i32 + 1;
    if let Some(rec) = sample.sectors.iter().find(|r| r.id == Some(game_id)) {
        if record_adoptable(rec) {
            return Some(rec);
        }
    }
    let mut nth = 0usize;
    for rec in &sample.sectors {
        if record_adoptable(rec) {
            if nth == leg_ix {
                return Some(rec);
            }
            nth += 1;
        }
    }
    None
}

fn leg_time_delta_for_record(
    sample: &GameClockSample,
    rec: &crate::game_clock_sync::GameClockSectorRecord,
) -> Option<f64> {
    let cur_t = rec.time_s.filter(|v| v.is_finite())?;
    let cur_idx = sample.sectors.iter().position(|r| std::ptr::eq(r, rec))?;
    let prev_t = if cur_idx == 0 {
        0.0
    } else {
        sample.sectors[..cur_idx]
            .iter()
            .rev()
            .find_map(|r| r.time_s.filter(|t| t.is_finite()))
            .unwrap_or(0.0)
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

/// True when UE4SS provides an adoptable split for leg `leg_ix` (0 = start→S1).
pub fn leg_ready_in_sample(sample: &GameClockSample, leg_ix: usize) -> bool {
    sector_leg_split_sec(sample, leg_ix).is_some()
}

/// UE4SS split for leg `leg_ix` from the corrector's last sample and/or adopter cache.
pub fn try_ue4ss_leg_split_sec(
    game_clock: &crate::game_clock_sync::GameClockCorrector,
    adopter: Option<&GameClockSectorAdopter>,
    leg_ix: usize,
    prior: &[f64],
) -> Option<f64> {
    if let Some(sample) = game_clock.last_game_sample() {
        if let Some(split) = sector_leg_split_sec(sample, leg_ix) {
            if !is_duplicate_of_prior(split, prior) {
                return Some(split);
            }
        }
    }
    adopter.and_then(|a| a.split_for_leg_checked(leg_ix, prior))
}

/// Leg split for stage leg index `leg_ix` (0 = Start→S1), mapped to game `sectors[]` / `id`.
pub fn sector_leg_split_sec(sample: &GameClockSample, leg_ix: usize) -> Option<f64> {
    let rec = sector_record_for_leg(sample, leg_ix)?;
    split_from_record(rec).or_else(|| leg_time_delta_for_record(sample, rec))
}

fn is_duplicate_of_prior(split: f64, prior_splits: &[f64]) -> bool {
    prior_splits
        .iter()
        .any(|p| p.is_finite() && (split - p).abs() < 0.001)
}

/// Leg split at Finish (S4 may appear in JSONL slightly after the gate cross).
///
/// Uses the same leg→`id` mapping as [`sector_leg_split_sec`]. No guess from `sectors.last()`:
/// adopting S3 into the S4 slot was a silent wrong-time bug.
pub fn sector_leg_split_sec_for_finish(
    sample: &GameClockSample,
    leg_ix: usize,
    _leg_count: usize,
) -> Option<f64> {
    sector_leg_split_sec(sample, leg_ix)
}

/// All stage leg splits from a Finish sample (`leg_count` = `sector_leg_count`).
pub fn all_stage_leg_splits_sec(sample: &GameClockSample, leg_count: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; leg_count];
    for i in 0..leg_count {
        out[i] = sector_leg_split_sec_for_finish(sample, i, leg_count);
    }
    out
}

/// Apply UE4SS sector times to a completed run; returns legs that changed.
pub fn apply_finish_sector_overrides(
    sample: &GameClockSample,
    session: &mut StageSectorSession,
    force: bool,
) -> Vec<FinishLegOverride> {
    let leg_count = session.markers.sector_leg_count;
    let mut out = Vec::new();
    for leg_ix in 0..leg_count {
        let Some(split) = sector_leg_split_sec_for_finish(sample, leg_ix, leg_count) else {
            continue;
        };
        if !(split.is_finite() && split > 0.05) {
            continue;
        }
        let prev = session.run.sector_secs[leg_ix];
        let changed = prev
            .map(|p| (p - split).abs() > 0.001)
            .unwrap_or(true);
        if !force && !changed {
            continue;
        }
        if leg_ix < session.run.sector_secs.len() {
            let prev_sec = prev.unwrap_or(split);
            session.run.sector_secs[leg_ix] = Some(split);
            if changed {
                out.push(FinishLegOverride {
                    session_si: 0,
                    leg_ix,
                    prev_sec,
                    ue4ss_sec: split,
                });
            }
        }
    }
    out
}

/// Poll JSONL after Finish and patch sector times when UE4SS data arrives.
pub fn drain_finish_overrides(
    adopter: &mut GameClockSectorAdopter,
    now: Instant,
    sessions: &mut [StageSectorSession],
) -> Vec<FinishLegOverride> {
    let Some(fp) = adopter.finish_pending.clone() else {
        return Vec::new();
    };
    if now < fp.next_poll {
        return Vec::new();
    }
    adopter.poll_force();
    let poll_iv = Duration::from_secs_f64(adopter.cfg.poll_interval_sec);
    let mut out = Vec::new();
    if let Some(sample) = adopter.cached.clone() {
        if let Some(session) = sessions.get_mut(fp.session_si) {
            let overrides = apply_finish_sector_overrides(&sample, session, true);
            for mut o in overrides {
                o.session_si = fp.session_si;
                out.push(o);
            }
            let all_present = (0..fp.leg_count).all(|leg_ix| {
                sector_leg_split_sec_for_finish(&sample, leg_ix, fp.leg_count).is_some()
            });
            if all_present {
                adopter.finish_pending = None;
                return out;
            }
        }
    }
    if now >= fp.deadline {
        if let Some(session) = sessions.get(fp.session_si) {
            let missing: Vec<usize> = (0..fp.leg_count)
                .filter(|&leg_ix| {
                    adopter
                        .cached
                        .as_ref()
                        .and_then(|s| sector_leg_split_sec_for_finish(s, leg_ix, fp.leg_count))
                        .is_none()
                })
                .map(|i| i + 1)
                .collect();
            if !missing.is_empty() {
                eprintln!(
                    "[{}] Sektor-Übernahme Finish: Sektor-Zeit (UE4SS) fehlt für S{missing:?} nach {:.1}s",
                    fp.label,
                    adopter.cfg.adopt_window_sec.max(3.0)
                );
            }
        }
        adopter.finish_pending = None;
        return out;
    }
    if let Some(fp) = adopter.finish_pending.as_mut() {
        fp.next_poll = now + poll_iv;
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
            t_wall_s: None,
            sample_kind: Some("full".into()),
        }
    }

    #[test]
    fn finish_leg4_none_when_only_three_game_sectors() {
        let sample = GameClockSample {
            race_time_s: Some(300.0),
            distance_m: None,
            race_time_valid: true,
            diff_time_s: None,
            position: None,
            phase: None,
            sectors: vec![
                GameClockSectorRecord {
                    id: Some(0),
                    time_s: Some(0.0),
                    split_s: Some(0.0),
                },
                GameClockSectorRecord {
                    id: Some(1),
                    time_s: Some(95.0),
                    split_s: Some(95.0),
                },
                GameClockSectorRecord {
                    id: Some(2),
                    time_s: Some(190.0),
                    split_s: Some(95.0),
                },
                GameClockSectorRecord {
                    id: Some(3),
                    time_s: Some(285.0),
                    split_s: Some(95.0),
                },
            ],
            sectors_source: None,
            sectors_debug: None,
            next_sector_index: Some(4),
            race_source: None,
            travel_track_id: None,
            travel_track_source: None,
            penalty_total_s: None,
            ghost_ref: None,
            game_x: None,
            game_z: None,
            t_process_ms: None,
            t_wall_s: None,
            sample_kind: Some("full".into()),
        };
        assert!(sector_leg_split_sec_for_finish(&sample, 3, 4).is_none());
    }

    #[test]
    fn finish_leg4_from_id4_row() {
        let sample = GameClockSample {
            race_time_s: Some(400.0),
            distance_m: None,
            race_time_valid: true,
            diff_time_s: None,
            position: None,
            phase: None,
            sectors: vec![
                GameClockSectorRecord {
                    id: Some(0),
                    time_s: Some(0.0),
                    split_s: Some(0.0),
                },
                GameClockSectorRecord {
                    id: Some(1),
                    time_s: Some(95.0),
                    split_s: Some(95.0),
                },
                GameClockSectorRecord {
                    id: Some(2),
                    time_s: Some(190.0),
                    split_s: Some(95.0),
                },
                GameClockSectorRecord {
                    id: Some(3),
                    time_s: Some(285.0),
                    split_s: Some(95.0),
                },
                GameClockSectorRecord {
                    id: Some(4),
                    time_s: Some(380.0),
                    split_s: Some(95.0),
                },
            ],
            sectors_source: None,
            sectors_debug: None,
            next_sector_index: Some(5),
            race_source: None,
            travel_track_id: None,
            travel_track_source: None,
            penalty_total_s: None,
            ghost_ref: None,
            game_x: None,
            game_z: None,
            t_process_ms: None,
            t_wall_s: None,
            sample_kind: Some("full".into()),
        };
        let s4 = sector_leg_split_sec_for_finish(&sample, 3, 4).unwrap();
        assert!((s4 - 95.0).abs() < 1e-6);
    }

    #[test]
    fn hafren_id0_placeholder_leg0_uses_id1() {
        let sample = GameClockSample {
            race_time_s: Some(178.0),
            distance_m: None,
            race_time_valid: true,
            diff_time_s: None,
            position: None,
            phase: None,
            sectors: vec![
                GameClockSectorRecord {
                    id: Some(0),
                    time_s: Some(0.0),
                    split_s: Some(0.0),
                },
                GameClockSectorRecord {
                    id: Some(1),
                    time_s: Some(109.220009),
                    split_s: Some(109.220009),
                },
            ],
            next_sector_index: Some(2),
            race_source: None,
            travel_track_id: None,
            travel_track_source: None,
            penalty_total_s: None,
            ghost_ref: None,
            game_x: None,
            game_z: None,
            t_process_ms: None,
            t_wall_s: None,
            sectors_source: None,
            sectors_debug: None,
            sample_kind: Some("full".into()),
        };
        let s1 = sector_leg_split_sec(&sample, 0).unwrap();
        assert!((s1 - 109.220009).abs() < 1e-6);
        assert!(sector_leg_split_sec(&sample, 1).is_none());
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
            t_wall_s: None,
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
            t_wall_s: None,
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
            finish_pending: None,
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
