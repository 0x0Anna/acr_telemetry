//! Shared library for acr_recorder and acr_export.

pub mod app_config;
pub mod config;
pub mod color_config;
pub mod export;
pub mod format_meta;
pub mod notes;
pub mod record;
pub mod recorder;
pub mod recording_position;
pub mod track_match_app;

// Workspace crates (timing / telemetry / pacenotes split for separate release).
pub use acr_telemetry::gis;
pub use acr_timing::{
    self, rtss_osd, split_beep, stage_overall_markers, subtiming, timing_db, track_spline_ref,
};
pub use acr_pacenote::{
    self, pacenote_ambiguous_overlay, pacenote_course, pacenote_voice, win_picker_input,
};
