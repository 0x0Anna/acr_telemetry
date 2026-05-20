//! Sector/session logic and reference-delta accumulation.
//!
//! Gate detection and ACC I/O remain in the host app until fully migrated here.

mod run_coordinator;
mod sector_plan;
mod sector_session;

pub use run_coordinator::RunCoordinator;
pub use sector_plan::sector_boundaries_from_labels;
pub use sector_session::{SectorBoundary, SectorSession, SectorSessionConfig};
