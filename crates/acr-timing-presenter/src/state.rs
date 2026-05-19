//! Presenter state from events.
//!
//! OSD: two lines — (1) last completed sector, (2) current sector live.

use std::time::Instant;

use acr_timing_protocol::{
    SectorCompleted, SectorIncomplete, SectorStarted, SubSplit, TimingEvent, TimingEventBody,
};

use crate::osd::{format_duration, format_sector_line};

#[derive(Debug, Default)]
pub struct PresenterState {
    pub live_line: Option<String>,
    last_completed: Option<SectorCompleted>,
    pub last_cum_delta_sec: f64,
    live_sector_index: Option<u32>,
    live_sector_started: Option<Instant>,
    live_sub_ids: Vec<i32>,
    live_sub_times_sec: Vec<Option<f64>>,
    /// Per-sub Δ vs reference (parallel to `live_sub_ids`).
    live_sub_delta_sec: Vec<Option<f64>>,
    /// Fastest complete reference sector time (full sector, not partial cum Δ).
    live_reference_tot_sec: Option<f64>,
    /// After Finish: keep OSD lines but stop live `tot` ticking.
    run_frozen: bool,
}

impl PresenterState {
    pub fn apply(&mut self, event: &TimingEvent) {
        match &event.body {
            TimingEventBody::TimingStarted(_) => {
                *self = Self::default();
            }
            TimingEventBody::SectorCompleted(s) => {
                self.last_completed = Some(s.clone());
                if !self.run_frozen {
                    self.live_line = None;
                    self.live_sector_index = None;
                    self.live_sector_started = None;
                    self.live_sub_ids.clear();
                    self.live_sub_times_sec.clear();
                    self.live_sub_delta_sec.clear();
                    self.live_reference_tot_sec = None;
                }
            }
            TimingEventBody::SectorIncomplete(s) => {
                self.last_completed = None;
                self.live_line = Some(format!(
                    "S{}~: (no subs) tot: {}",
                    s.sector_index + 1,
                    format_duration(s.tot_sec)
                ));
                if !self.run_frozen {
                    self.live_line = None;
                    self.live_sector_index = None;
                    self.live_sector_started = None;
                }
            }
            TimingEventBody::SectorStarted(s) => {
                self.run_frozen = false;
                let ref_tot = s.reference_tot_sec;
                self.live_reference_tot_sec = ref_tot.is_finite().then_some(ref_tot);
                self.begin_live_sector(
                    s.sector_index,
                    Some(&s.reference_sub_ids),
                    None,
                );
            }
            TimingEventBody::RunFinished(_) => {
                self.run_frozen = true;
                self.live_sector_started = None;
                if let Some(c) = self.last_completed.clone() {
                    self.live_line = Some(format_completed(&c, false));
                }
            }
            TimingEventBody::SubSplit(s) => {
                self.last_cum_delta_sec = s.cum_delta_sec;
                if self.live_sector_index != Some(s.sector_index) {
                    self.begin_live_sector(s.sector_index, None, None);
                    self.live_reference_tot_sec = None;
                }
                if let Some(pos) = self.live_sub_ids.iter().position(|id| *id == s.sub_id) {
                    if let Some(t) = self.live_sub_times_sec.get_mut(pos) {
                        *t = Some(s.leg_time_sec);
                    }
                    if pos < self.live_sub_delta_sec.len() {
                        self.live_sub_delta_sec[pos] = s.delta_i_sec;
                    }
                } else {
                    self.live_sub_ids.push(s.sub_id);
                    self.live_sub_times_sec.push(Some(s.leg_time_sec));
                    self.live_sub_delta_sec.push(s.delta_i_sec);
                }
            }
            _ => {}
        }
    }

    /// Reset lower line for `sector_index` (display S{n} with n = sector_index + 1).
    fn begin_live_sector(
        &mut self,
        sector_index: u32,
        sub_ids: Option<&[i32]>,
        sub_times: Option<&[Option<f64>]>,
    ) {
        self.live_sector_index = Some(sector_index);
        self.live_sector_started = Some(Instant::now());
        self.last_cum_delta_sec = 0.0;
        match (sub_ids, sub_times) {
            (Some(ids), Some(times)) => {
                self.live_sub_ids = ids.to_vec();
                self.live_sub_times_sec = times.to_vec();
                self.live_sub_delta_sec = vec![None; ids.len()];
            }
            (Some(ids), None) => {
                self.live_sub_ids = ids.to_vec();
                self.live_sub_times_sec = vec![None; ids.len()];
                self.live_sub_delta_sec = vec![None; ids.len()];
            }
            _ => {
                self.live_sub_ids.clear();
                self.live_sub_times_sec.clear();
                self.live_sub_delta_sec.clear();
            }
        }
        self.live_line = Some(format!("S{}: …", sector_index + 1));
    }

    /// Recompute lower line `tot` from wall clock (call each OSD frame between sub events).
    pub fn refresh_live(&mut self, rtss_colors: bool) {
        if self.run_frozen {
            return;
        }
        let Some(sector_index) = self.live_sector_index else {
            return;
        };
        let Some(_) = self.live_sector_started else {
            return;
        };
        let tot_sec = self
            .live_sector_started
            .map(|t| Instant::now().duration_since(t).as_secs_f64())
            .unwrap_or(0.0);
        self.live_line = Some(format_sector_line(
            sector_index,
            self.last_cum_delta_sec,
            &self.live_sub_ids,
            &self.live_sub_times_sec,
            &self.live_sub_delta_sec,
            self.live_reference_tot_sec,
            tot_sec,
            false,
            rtss_colors,
        ));
    }

    /// Upper = completed, lower = live (always two lines once timing has started).
    pub fn osd_lines(&mut self, rtss_colors: bool) -> Vec<String> {
        self.refresh_live(rtss_colors);
        let mut out = Vec::new();
        if let Some(c) = &self.last_completed {
            out.push(format_completed(c, rtss_colors));
        }
        if let Some(l) = &self.live_line {
            out.push(l.clone());
        }
        out
    }
}

fn format_completed(s: &SectorCompleted, rtss_colors: bool) -> String {
    let ref_tot = s.reference_tot_sec;
    format_sector_line(
        s.sector_index,
        s.cum_delta_sec,
        &s.sub_ids,
        &s.sub_times_sec,
        &s.sub_delta_sec,
        ref_tot.is_finite().then_some(ref_tot),
        s.tot_sec,
        false,
        rtss_colors,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use acr_timing_protocol::TimingEvent;

    #[test]
    fn timing_started_clears_stale_lines() {
        let mut p = PresenterState::default();
        p.live_line = Some("S10: stale".into());
        p.apply(&TimingEvent::new(TimingEventBody::TimingStarted(
            acr_timing_protocol::TimingStarted {
                reference_track: "t".into(),
                stage_slug: "s".into(),
            },
        )));
        assert!(p.osd_lines(false).is_empty());
    }

    #[test]
    fn completed_then_advances_live_to_next_sector_line() {
        let mut p = PresenterState::default();
        p.apply(&TimingEvent::new(TimingEventBody::SectorStarted(SectorStarted {
            sector_index: 0,
            reference_run_id: None,
            reference_sub_ids: vec![1, 2],
            reference_sub_times_sec: vec![1.0, 2.0],
            reference_tot_sec: 3.0,
        })));
        assert_eq!(p.osd_lines(false).len(), 1);
        std::thread::sleep(std::time::Duration::from_millis(20));
        p.apply(&TimingEvent::new(TimingEventBody::SectorCompleted(SectorCompleted {
            sector_index: 0,
            cum_delta_sec: 0.5,
            tot_sec: 90.0,
            sub_ids: vec![1, 2],
            sub_times_sec: vec![Some(1.0), Some(2.0)],
            sub_delta_sec: vec![Some(0.5), Some(-0.2)],
            reference_tot_sec: 3.0,
        })));
        let lines = p.osd_lines(false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("S1:"));
        let colored = p.osd_lines(true);
        assert!(colored[0].contains("<C=ff0000>"));
        p.apply(&TimingEvent::new(TimingEventBody::SectorStarted(SectorStarted {
            sector_index: 1,
            reference_run_id: None,
            reference_sub_ids: vec![3],
            reference_sub_times_sec: vec![4.0],
            reference_tot_sec: 4.0,
        })));
        let lines = p.osd_lines(false);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].starts_with("S2:"));
        p.apply(&TimingEvent::new(TimingEventBody::RunFinished(
            acr_timing_protocol::RunFinished {
                reference_track: "t".into(),
                stage_slug: "s".into(),
            },
        )));
        assert!(p.run_frozen);
    }
}
