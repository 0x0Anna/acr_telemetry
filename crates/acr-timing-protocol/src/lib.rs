//! Versioned timing events published by [`acr_timing_engine`] and consumed by
//! [`acr_timing_presenter`] and [`acr_timing_store`].
//!
//! Transport (in-process bus, UDP) is separate from payload semantics.

pub mod bus;
pub mod events;

pub use bus::{EventReceiver, EventSender};
pub use events::{
    RouteIdentified, RunFinished, RunInvalidated, SectorCompleted, SectorIncomplete,
    SectorStarted, SubSplit, TimingEvent, TimingEventBody, TimingStarted, SCHEMA_VERSION,
};
