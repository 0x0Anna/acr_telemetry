//! Read rkyv telemetry files (version-aware). Implementation: [`rkyv_format`].

use std::path::Path;

use crate::record::{GraphicsRecord, PhysicsRecord};

/// Read all physics records (v1 archives upgraded to current `PhysicsRecord`).
pub fn read_rkyv(path: impl AsRef<Path>) -> std::io::Result<(u32, Vec<PhysicsRecord>)> {
    let (hz, _ver, records) = super::rkyv_format::read_physics(path)?;
    Ok((hz, records))
}

/// Read all graphics records (v1 archives upgraded to current `GraphicsRecord`).
pub fn read_graphics_rkyv(path: impl AsRef<Path>) -> std::io::Result<(u32, Vec<GraphicsRecord>)> {
    let (hz, _ver, records) = super::rkyv_format::read_graphics(path)?;
    Ok((hz, records))
}
