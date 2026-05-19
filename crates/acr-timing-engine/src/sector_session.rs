//! One main sector within a run: reference snapshot, per-sub deltas, completion.

use std::collections::HashMap;
use std::time::Instant;

use acr_timing_protocol::{
    SectorCompleted, SectorIncomplete, SectorStarted, SubSplit, TimingEvent, TimingEventBody,
};
use acr_timing_store::ReferenceSnapshot;

#[derive(Debug, Clone)]
pub struct SectorBoundary {
    pub sector_index: u32,
    /// Sub gate ids between this sector start and end (route order).
    pub sub_ids: Vec<i32>,
}

#[derive(Debug, Clone)]
pub struct SectorSessionConfig {
    pub reference_track: String,
    pub car: String,
    pub stage_slug: String,
}

/// Active sector timing vs frozen reference subs (by sub_id).
#[derive(Debug)]
pub struct SectorSession {
    cfg: SectorSessionConfig,
    sector_index: u32,
    sub_ids_order: Vec<i32>,
    ref_time_by_id: HashMap<i32, f64>,
    reference_tot_sec: f64,
    reference_run_id: Option<i64>,
    hit_times: HashMap<i32, f64>,
    cum_delta_sec: f64,
    sector_started_at: Instant,
}

impl SectorSession {
    pub fn start(
        cfg: SectorSessionConfig,
        boundary: &SectorBoundary,
        reference: Option<ReferenceSnapshot>,
    ) -> (Self, TimingEvent) {
        let (ref_time_by_id, reference_tot_sec, reference_run_id, sub_ids_order) =
            match reference {
                Some(r) => {
                    let mut map = HashMap::new();
                    for (&id, &t) in r.sub_ids.iter().zip(r.sub_times_sec.iter()) {
                        if t.is_finite() {
                            map.insert(id, t);
                        }
                    }
                    (
                        map,
                        r.tot_sec,
                        Some(r.run_id),
                        r.sub_ids.clone(),
                    )
                }
                None => {
                    let mut map = HashMap::new();
                    for &id in &boundary.sub_ids {
                        map.insert(id, f64::NAN);
                    }
                    (
                        map,
                        f64::NAN,
                        None,
                        boundary.sub_ids.clone(),
                    )
                }
            };

        let session = Self {
            cfg,
            sector_index: boundary.sector_index,
            sub_ids_order,
            ref_time_by_id,
            reference_tot_sec,
            reference_run_id,
            hit_times: HashMap::new(),
            cum_delta_sec: 0.0,
            sector_started_at: Instant::now(),
        };

        let ev = TimingEvent::new(TimingEventBody::SectorStarted(SectorStarted {
            sector_index: boundary.sector_index,
            reference_run_id,
            reference_sub_ids: session.sub_ids_order.clone(),
            reference_sub_times_sec: session
                .sub_ids_order
                .iter()
                .map(|id| session.ref_time_by_id.get(id).copied().unwrap_or(f64::NAN))
                .collect(),
            reference_tot_sec: session.reference_tot_sec,
        }));

        (session, ev)
    }

    /// Sub gate crossed; always records time; updates cum delta only when reference has this sub_id.
    pub fn on_sub_split(&mut self, sub_id: i32, leg_time_sec: f64) -> TimingEvent {
        self.hit_times.insert(sub_id, leg_time_sec);
        let delta_i = self
            .ref_time_by_id
            .get(&sub_id)
            .copied()
            .filter(|r| r.is_finite())
            .map(|r| {
                let d = leg_time_sec - r;
                self.cum_delta_sec += d;
                d
            });

        TimingEvent::new(TimingEventBody::SubSplit(SubSplit {
            sector_index: self.sector_index,
            sub_id,
            leg_time_sec,
            delta_i_sec: delta_i,
            cum_delta_sec: self.cum_delta_sec,
        }))
    }

    pub fn finish(self, ended_at: Instant) -> (TimingEvent, acr_timing_store::SectorRunRecord) {
        let subs: Vec<_> = self
            .sub_ids_order
            .iter()
            .map(|id| {
                let time = self.hit_times.get(id).copied();
                let delta_i = time.and_then(|t| {
                    self.ref_time_by_id
                        .get(id)
                        .filter(|r| r.is_finite())
                        .map(|r| t - r)
                });
                acr_timing_store::SubSplitRecord {
                    sub_id: *id,
                    time_sec: time,
                    delta_i,
                }
            })
            .collect();

        let sub_sum_sec: f64 = subs.iter().filter_map(|s| s.time_sec).sum();
        let wall_tot_sec = ended_at
            .duration_since(self.sector_started_at)
            .as_secs_f64();
        let any_sub = subs.iter().any(|s| s.time_sec.is_some());
        let all_subs = subs.iter().all(|s| s.time_sec.is_some());
        // Match stage timing: sector total = sum of leg splits when every slot was hit.
        let tot_sec = if all_subs && sub_sum_sec > 0.05 {
            sub_sum_sec
        } else {
            wall_tot_sec
        };
        if all_subs && (wall_tot_sec - sub_sum_sec).abs() > 0.25 {
            eprintln!(
                "sector S{}: tot={tot_sec:.3}s (legs) wall={wall_tot_sec:.3}s gap={:+.3}s",
                self.sector_index + 1,
                wall_tot_sec - sub_sum_sec,
            );
        }
        let is_complete = any_sub && all_subs;

        let rec = acr_timing_store::SectorRunRecord {
            reference_track: self.cfg.reference_track.clone(),
            car: self.cfg.car.clone(),
            stage_slug: self.cfg.stage_slug.clone(),
            sector_index: self.sector_index,
            tot_sec,
            cum_delta_sec: self.cum_delta_sec,
            is_complete,
            invalidated: false,
            subs,
        };

        let ev = if any_sub {
            let sub_times_sec: Vec<Option<f64>> = self
                .sub_ids_order
                .iter()
                .map(|id| self.hit_times.get(id).copied())
                .collect();
            TimingEvent::new(TimingEventBody::SectorCompleted(SectorCompleted {
                sector_index: self.sector_index,
                cum_delta_sec: self.cum_delta_sec,
                tot_sec,
                sub_ids: self.sub_ids_order.clone(),
                sub_times_sec,
                reference_tot_sec: self.reference_tot_sec,
            }))
        } else {
            TimingEvent::new(TimingEventBody::SectorIncomplete(SectorIncomplete {
                sector_index: self.sector_index,
                tot_sec,
            }))
        };

        (ev, rec)
    }

    pub fn cum_delta_sec(&self) -> f64 {
        self.cum_delta_sec
    }

    pub fn sector_index(&self) -> u32 {
        self.sector_index
    }

    pub fn sub_ids_order(&self) -> &[i32] {
        &self.sub_ids_order
    }
}
