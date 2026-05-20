//! Presenter state from events.
//!
//! OSD: two lines — (1) last completed sector, (2) current sector live or run summary.

use std::time::Instant;

use acr_timing::delta_display::{DeltaColorStyle, DeltaDisplayConfig};
use acr_timing_protocol::{SectorCompleted, SectorStarted, TimingEvent, TimingEventBody};

use crate::osd::{format_duration, format_sector_line, format_track_completed_line};

#[derive(Debug, Default)]
pub struct PresenterState {
    pub live_line: Option<String>,
    last_completed: Option<SectorCompleted>,
    pub last_cum_delta_sec: f64,
    live_sector_index: Option<u32>,
    live_sector_started: Option<Instant>,
    live_sub_ids: Vec<i32>,
    live_sub_times_sec: Vec<Option<f64>>,
    live_sub_delta_sec: Vec<Option<f64>>,
    live_reference_tot_sec: Option<f64>,
    live_reference_sub_times_sec: Vec<f64>,
    run_frozen: bool,
    run_tot_sum_sec: f64,
    run_ref_tot_sum_sec: f64,
    run_cum_delta_sec: f64,
    run_sector_count: u32,
    completed_sectors: Vec<SectorCompleted>,
    finish_carousel_at: Option<Instant>,
}

impl PresenterState {
    pub fn apply(&mut self, event: &TimingEvent) {
        match &event.body {
            TimingEventBody::TimingStarted(_) => {
                *self = Self::default();
            }
            TimingEventBody::SectorCompleted(s) => {
                self.run_tot_sum_sec += s.tot_sec;
                if s.reference_tot_sec.is_finite() {
                    self.run_ref_tot_sum_sec += s.reference_tot_sec;
                }
                self.run_cum_delta_sec += s.cum_delta_sec;
                self.run_sector_count += 1;
                self.completed_sectors.push(s.clone());
                self.last_completed = Some(s.clone());
                if !self.run_frozen {
                    self.live_line = None;
                    self.live_sector_index = None;
                    self.live_sector_started = None;
                    self.live_sub_ids.clear();
                    self.live_sub_times_sec.clear();
                    self.live_sub_delta_sec.clear();
                    self.live_reference_tot_sec = None;
                    self.live_reference_sub_times_sec.clear();
                }
            }
            TimingEventBody::SectorIncomplete(s) => {
                self.last_completed = None;
                if self.run_frozen {
                    return;
                }
                self.live_line = Some(format!(
                    "S{}~: (no subs) tot: {}",
                    s.sector_index + 1,
                    format_duration(s.tot_sec)
                ));
            }
            TimingEventBody::SectorStarted(s) => {
                if self.run_frozen {
                    return;
                }
                let ref_tot = s.reference_tot_sec;
                self.live_reference_tot_sec = ref_tot.is_finite().then_some(ref_tot);
                self.live_reference_sub_times_sec = s.reference_sub_times_sec.clone();
                self.begin_live_sector(
                    s.sector_index,
                    Some(&s.reference_sub_ids),
                    None,
                );
            }
            TimingEventBody::RunFinished(_) => {
                self.run_frozen = true;
                self.finish_carousel_at = Some(Instant::now());
                self.live_sector_index = None;
                self.live_sector_started = None;
                self.live_sub_ids.clear();
                self.live_sub_times_sec.clear();
                self.live_sub_delta_sec.clear();
            }
            TimingEventBody::SubSplit(s) => {
                if self.run_frozen {
                    return;
                }
                self.last_cum_delta_sec = s.cum_delta_sec;
                if self.live_sector_index != Some(s.sector_index) {
                    self.begin_live_sector(s.sector_index, None, None);
                    self.live_reference_tot_sec = None;
                    self.live_reference_sub_times_sec.clear();
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
                    self.live_reference_sub_times_sec.push(f64::NAN);
                }
            }
            _ => {}
        }
    }

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

    fn refresh_run_completed_line(&mut self, rtss: bool, delta_style: &DeltaColorStyle) {
        if !self.run_frozen || self.run_sector_count == 0 {
            return;
        }
        self.live_line = Some(format_track_completed_line(
            self.run_tot_sum_sec,
            self.run_ref_tot_sum_sec,
            self.run_cum_delta_sec,
            rtss,
            delta_style,
        ));
    }

    pub fn refresh_live(&mut self, rtss: bool, delta_style: &DeltaColorStyle) {
        if self.run_frozen {
            self.refresh_run_completed_line(rtss, delta_style);
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
            rtss,
            delta_style,
        ));
    }

    /// Sum of completed sector `tot` values (OSD „Track completed cum“).
    pub fn track_completed_cum_sec(&self) -> f64 {
        self.run_tot_sum_sec
    }

    fn carousel_sector<'a>(
        &'a self,
        recap_sec: f64,
    ) -> Option<&'a SectorCompleted> {
        if recap_sec <= 0.0 || self.completed_sectors.is_empty() {
            return self.last_completed.as_ref();
        }
        let started = self.finish_carousel_at?;
        let ix = (started.elapsed().as_secs_f64() / recap_sec).floor() as usize
            % self.completed_sectors.len();
        self.completed_sectors.get(ix)
    }

    pub fn osd_lines(&mut self, rtss: bool, display: &DeltaDisplayConfig) -> Vec<String> {
        let delta_style = &display.colors;
        self.refresh_live(rtss, delta_style);
        let mut out = Vec::new();
        let completed = if self.run_frozen {
            self.carousel_sector(display.sector_recap_sec)
        } else {
            self.last_completed.as_ref()
        };
        if let Some(c) = completed {
            out.push(format_completed(c, rtss, delta_style));
        }
        if let Some(l) = &self.live_line {
            out.push(l.clone());
        }
        out
    }
}

fn format_completed(s: &SectorCompleted, rtss: bool, delta_style: &DeltaColorStyle) -> String {
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
        rtss,
        delta_style,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use acr_timing_protocol::{SectorStarted, TimingEvent};

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
        assert!(p
            .osd_lines(false, &DeltaDisplayConfig::default())
            .is_empty());
    }

    #[test]
    fn run_finished_shows_track_completed_not_next_sector() {
        let mut p = PresenterState::default();
        p.apply(&TimingEvent::new(TimingEventBody::SectorStarted(SectorStarted {
            sector_index: 0,
            reference_run_id: None,
            reference_sub_ids: vec![1],
            reference_sub_times_sec: vec![1.0],
            reference_tot_sec: 90.0,
        })));
        p.apply(&TimingEvent::new(TimingEventBody::SectorCompleted(SectorCompleted {
            sector_index: 0,
            cum_delta_sec: 0.5,
            tot_sec: 90.0,
            sub_ids: vec![1],
            sub_times_sec: vec![Some(1.0)],
            sub_delta_sec: vec![Some(0.5)],
            reference_tot_sec: 89.0,
        })));
        p.apply(&TimingEvent::new(TimingEventBody::RunFinished(
            acr_timing_protocol::RunFinished {
                reference_track: "t".into(),
                stage_slug: "s".into(),
            },
        )));
        p.apply(&TimingEvent::new(TimingEventBody::SectorStarted(SectorStarted {
            sector_index: 1,
            reference_run_id: None,
            reference_sub_ids: vec![2],
            reference_sub_times_sec: vec![2.0],
            reference_tot_sec: 80.0,
        })));
        let lines = p.osd_lines(false, &DeltaDisplayConfig::default());
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("S1:"));
        assert!(lines[1].contains("Track completed"));
        assert!(lines[1].contains("cum:"));
        assert!(lines[1].contains("delta:"));
        assert!(!lines[1].starts_with("S2:"));
    }

    #[test]
    fn finish_carousel_cycles_sectors() {
        let mut p = PresenterState::default();
        for (ix, tot) in [(0, 90.0), (1, 80.0), (2, 70.0)] {
            p.apply(&TimingEvent::new(TimingEventBody::SectorCompleted(SectorCompleted {
                sector_index: ix,
                cum_delta_sec: 0.0,
                tot_sec: tot,
                sub_ids: vec![ix as i32 + 1],
                sub_times_sec: vec![Some(1.0)],
                sub_delta_sec: vec![Some(0.0)],
                reference_tot_sec: tot,
            })));
        }
        p.apply(&TimingEvent::new(TimingEventBody::RunFinished(
            acr_timing_protocol::RunFinished {
                reference_track: "t".into(),
                stage_slug: "s".into(),
            },
        )));
        let mut cfg = DeltaDisplayConfig::default();
        cfg.sector_recap_sec = 5.0;
        let lines = p.osd_lines(false, &cfg);
        assert!(lines[0].starts_with("S1:"));
        p.finish_carousel_at = Some(Instant::now() - std::time::Duration::from_secs(6));
        let lines = p.osd_lines(false, &cfg);
        assert!(lines[0].starts_with("S2:"));
        p.finish_carousel_at = Some(Instant::now() - std::time::Duration::from_secs(11));
        let lines = p.osd_lines(false, &cfg);
        assert!(lines[0].starts_with("S3:"));
    }
}
