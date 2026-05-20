//! Event payloads — see module docs on each variant.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// Top-level envelope on the wire or bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingEvent {
    pub schema_version: u32,
    #[serde(flatten)]
    pub body: TimingEventBody,
}

impl TimingEvent {
    pub fn new(body: TimingEventBody) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            body,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TimingEventBody {
    /// Base route matched (spline length, direction of sub-chain, stage slug).
    RouteIdentified(RouteIdentified),
    /// Wall-clock timing active (~1 m forward from start rest position).
    TimingStarted(TimingStarted),
    /// Main sector entered; reference snapshot frozen for this sector in this run.
    SectorStarted(SectorStarted),
    /// Sub gate crossed (silent CP); times and deltas are for this `sub_id` only.
    SubSplit(SubSplit),
    /// Sector end gate; all reference sub slots present with times.
    SectorCompleted(SectorCompleted),
    /// Sector end but no sub split was recorded in this sector.
    SectorIncomplete(SectorIncomplete),
    /// Run ended (finish or abort).
    RunFinished(RunFinished),
    /// Run invalidated (crash / reset) — still logged, not a reference candidate.
    RunInvalidated(RunInvalidated),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteIdentified {
    pub reference_track: String,
    pub stage_slug: String,
    pub route_length_m: f64,
    /// Monotonic sub gate ids along the stage (defines comparison order for display).
    pub sub_ids_in_order: Vec<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingStarted {
    pub reference_track: String,
    pub stage_slug: String,
    /// Composite stage reference (sum of sector bests), when configured.
    pub reference_stage_tot_sec: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectorStarted {
    pub sector_index: u32,
    pub reference_run_id: Option<i64>,
    /// Sub ids in display order (subset of route); reference times aligned by id.
    pub reference_sub_ids: Vec<i32>,
    pub reference_sub_times_sec: Vec<f64>,
    /// Sum of reference sub times for this sector (convenience for UI).
    pub reference_tot_sec: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubSplit {
    pub sector_index: u32,
    pub sub_id: i32,
    pub leg_time_sec: f64,
    /// `leg_time_sec - reference_time(sub_id)` when reference exists for this id.
    pub delta_i_sec: Option<f64>,
    /// Sum of `delta_i` over subs actually crossed this sector this run.
    pub cum_delta_sec: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectorCompleted {
    pub sector_index: u32,
    pub cum_delta_sec: f64,
    pub tot_sec: f64,
    /// Parallel to `reference_sub_ids` at sector start: `None` = missed sub.
    pub sub_ids: Vec<i32>,
    pub sub_times_sec: Vec<Option<f64>>,
    /// Per-sub Δ vs reference (`time - ref`), aligned with `sub_ids`.
    pub sub_delta_sec: Vec<Option<f64>>,
    pub reference_tot_sec: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectorIncomplete {
    pub sector_index: u32,
    pub tot_sec: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunFinished {
    pub reference_track: String,
    pub stage_slug: String,
    pub reference_stage_tot_sec: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInvalidated {
    pub reason: String,
}
