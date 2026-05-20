//! Orchestrates sector sessions, store snapshots, and event publish.

use std::time::Instant;

use acr_timing_protocol::{
    EventSender, RunFinished, TimingEvent, TimingEventBody, TimingStarted,
};
use acr_timing_store::{ReferenceStore, ReferenceTimeMode};

use crate::sector_plan::sector_boundaries_from_labels;
use crate::sector_session::{SectorBoundary, SectorSession, SectorSessionConfig};

pub struct RunCoordinator {
    bus: EventSender,
    store: ReferenceStore,
    cfg: SectorSessionConfig,
    reference_mode: ReferenceTimeMode,
    reference_stage_tot_sec: Option<f64>,
    sectors: Vec<SectorBoundary>,
    sector_cursor: usize,
    session: Option<SectorSession>,
    timing_active: bool,
}

impl RunCoordinator {
    pub fn new(
        bus: EventSender,
        store: ReferenceStore,
        cfg: SectorSessionConfig,
        reference_mode: ReferenceTimeMode,
    ) -> Self {
        Self {
            bus,
            store,
            cfg,
            reference_mode,
            reference_stage_tot_sec: None,
            sectors: Vec::new(),
            sector_cursor: 0,
            session: None,
            timing_active: false,
        }
    }

    pub fn set_reference_mode(&mut self, mode: ReferenceTimeMode) {
        self.reference_mode = mode;
    }

    pub fn set_route(&mut self, ordered_labels: &[(i32, String)]) {
        self.sectors = sector_boundaries_from_labels(ordered_labels);
        self.sector_cursor = 0;
        self.session = None;
    }

    pub fn set_car(&mut self, car: impl Into<String>) {
        self.cfg.car = car.into();
    }

    pub fn reset_run(&mut self) {
        self.timing_active = false;
        self.sector_cursor = 0;
        self.session = None;
    }

    pub fn timing_started(&mut self) {
        self.timing_active = true;
        self.sector_cursor = 0;
        self.session = None;
        let sector_count = self.sectors.len() as u32;
        self.reference_stage_tot_sec = self
            .store
            .best_stage_tot_sec(
                &self.cfg.reference_track,
                &self.cfg.car,
                &self.cfg.stage_slug,
                sector_count,
            )
            .ok()
            .flatten()
            .filter(|t| t.is_finite() && *t > 0.05);
        eprintln!(
            "modular: reference mode {} stage_ref={}",
            self.reference_mode.as_str(),
            self.reference_stage_tot_sec
                .map(|t| format!("{t:.3}s"))
                .unwrap_or_else(|| "—".into())
        );
        self.bus.publish(TimingEvent::new(TimingEventBody::TimingStarted(
            TimingStarted {
                reference_track: self.cfg.reference_track.clone(),
                stage_slug: self.cfg.stage_slug.clone(),
                reference_stage_tot_sec: self.reference_stage_tot_sec,
            },
        )));
        self.begin_sector_if_needed();
    }

    fn begin_sector_if_needed(&mut self) {
        if !self.timing_active {
            return;
        }
        if self.session.is_some() {
            return;
        }
        let Some(boundary) = self.sectors.get(self.sector_cursor).cloned() else {
            return;
        };
        let reference = self
            .store
            .resolve_reference(
                self.reference_mode,
                &self.cfg.reference_track,
                &self.cfg.car,
                &self.cfg.stage_slug,
                boundary.sector_index,
                &boundary.sub_ids,
            )
            .ok()
            .flatten();
        let (session, ev) = SectorSession::start(self.cfg.clone(), &boundary, reference);
        eprintln!(
            "modular: sector S{} start ({} subs)",
            boundary.sector_index + 1,
            boundary.sub_ids.len()
        );
        self.bus.publish(ev);
        self.session = Some(session);
    }

    /// Main sector end marker crossed (`Sector 2`, `Finish`, …).
    pub fn on_main_sector_end(&mut self, sector_label: &str, ended_at: Instant) {
        let completed_index = match sector_label {
            "Finish" => self.sectors.last().map(|b| b.sector_index),
            _ => sector_label
                .strip_prefix("Sector ")
                .and_then(|s| s.parse::<u32>().ok())
                .map(|n| n.saturating_sub(1)),
        };
        let Some(completed_index) = completed_index else {
            return;
        };
        while self.session.as_ref().is_some_and(|s| s.sector_index() < completed_index) {
            self.finish_current_sector(ended_at);
            if self.sector_cursor < self.sectors.len() {
                self.sector_cursor += 1;
            }
            self.begin_sector_if_needed();
        }
        if self.session.as_ref().map(|s| s.sector_index()) == Some(completed_index) {
            self.finish_current_sector(ended_at);
        }
        while self.sector_cursor < self.sectors.len()
            && self.sectors[self.sector_cursor].sector_index <= completed_index
        {
            self.sector_cursor += 1;
        }
        if sector_label == "Finish" {
            self.timing_active = false;
            self.bus.publish(TimingEvent::new(TimingEventBody::RunFinished(RunFinished {
                reference_track: self.cfg.reference_track.clone(),
                stage_slug: self.cfg.stage_slug.clone(),
                reference_stage_tot_sec: self.reference_stage_tot_sec,
            })));
        } else {
            self.begin_sector_if_needed();
        }
    }

    pub fn on_sub_cross(&mut self, sub_id: i32, leg_time_sec: f64) {
        if !self.timing_active {
            return;
        }
        self.begin_sector_if_needed();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.sub_ids_order().contains(&sub_id) {
            return;
        }
        let ev = session.on_sub_split(sub_id, leg_time_sec);
        self.bus.publish(ev);
    }

    fn finish_current_sector(&mut self, ended_at: Instant) {
        let Some(session) = self.session.take() else {
            return;
        };
        let sector_ix = session.sector_index();
        let (ev, rec) = session.finish(ended_at);
        if let TimingEventBody::SectorCompleted(s) = &ev.body {
            eprintln!(
                "modular: sector S{} done tot={:.3}s cum_d={:+.3}s",
                sector_ix + 1,
                s.tot_sec,
                s.cum_delta_sec
            );
        } else if let TimingEventBody::SectorIncomplete(s) = &ev.body {
            eprintln!(
                "modular: sector S{} done (no subs) tot={:.3}s",
                sector_ix + 1,
                s.tot_sec
            );
        }
        self.bus.publish(ev);
        if let Err(e) = self.store.insert_sector_run(&rec) {
            eprintln!("timing_store: {e}");
        }
    }

    pub fn finish_run(&mut self, ended_at: Instant) {
        self.finish_current_sector(ended_at);
        self.timing_active = false;
    }
}
