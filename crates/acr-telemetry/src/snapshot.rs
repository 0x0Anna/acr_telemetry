//! Single-poll ACC telemetry frame for live timing and pacenote tools.

use acc_shared_memory_rs::maps::{GraphicsMap, PhysicsMap, StaticsMap};
use acc_shared_memory_rs::{ACCError, ACCSharedMemory, ACCMap};

/// One synchronized read of physics, graphics, and statics shared memory.
#[derive(Debug, Clone)]
pub struct AccSnapshot {
    pub physics: PhysicsMap,
    pub graphics: GraphicsMap,
    pub statics: StaticsMap,
}

/// Opens ACC shared memory maps once; call [`poll`](Self::poll) each frame.
pub struct TelemetryReader {
    acc: ACCSharedMemory,
}

impl TelemetryReader {
    pub fn new() -> Result<Self, ACCError> {
        Ok(Self {
            acc: ACCSharedMemory::new()?,
        })
    }

    /// Returns `None` when no new physics packet is available (same as upstream `read_shared_memory`).
    pub fn poll(&mut self) -> Result<Option<AccSnapshot>, ACCError> {
        let Some(ACCMap {
            physics,
            graphics,
            statics,
            timestamp: _,
        }) = self.acc.read_shared_memory()?
        else {
            return Ok(None);
        };
        Ok(Some(AccSnapshot {
            physics,
            graphics,
            statics,
        }))
    }
}

impl AccSnapshot {
    pub fn player_world_xz(&self) -> Option<(f64, f64)> {
        let pid = self.graphics.player_car_id;
        self.graphics
            .car_coordinates
            .iter()
            .zip(&self.graphics.car_id)
            .find(|(_, id)| **id == pid)
            .map(|(c, _)| (c.x as f64, c.z as f64))
    }

    pub fn track_name(&self) -> &str {
        self.statics.track.trim()
    }

    pub fn car_model(&self) -> &str {
        self.statics.car_model.trim()
    }

    /// ACC static shared memory: total track spline length (metres).
    pub fn track_spline_length_m(&self) -> f32 {
        self.statics.track_spline_length
    }
}
