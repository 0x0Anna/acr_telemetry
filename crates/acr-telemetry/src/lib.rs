//! Shared ACC telemetry read path and GIS coordinate helpers.

pub mod gis;
pub mod paths;
pub mod snapshot;

pub use snapshot::{AccSnapshot, TelemetryReader};
