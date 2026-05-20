//! Consumes [`acr_timing_protocol`] events and drives overlay + audio feedback.

mod osd;
mod state;

pub use osd::compose_osd_message;
pub use state::PresenterState;
