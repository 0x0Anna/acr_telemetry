//! Sector timing, split database, sub-timing markers, and RTSS overlay helpers.

pub mod physics_wheel;
pub mod rtss_osd;
pub mod sector_leg_stats;
pub mod split_beep;
pub mod stage_overall_markers;
pub mod stage_sector_timing;
pub mod stage_timing_config;
pub mod subsection_split_html;
pub mod subtiming;
pub mod timing_blame;
pub mod timing_debug;
pub mod cumulative_sector_timing;
pub mod cumulative_timing_config;
pub mod delta_display;
pub mod osd_template;
pub mod reference_times;
pub mod timing_config_file;
pub mod timing_correlation;
pub mod timing_db;
pub mod run_timing_clock;
pub mod timing_frame_quality;
pub mod timing_pb;
pub mod timing_voice;
pub mod timing_sectors;
pub mod track_spline_ref;

pub use acr_telemetry::gis;
pub use delta_display::{
    DeltaColorStyle, DeltaDisplayConfig, DeltaDisplayConfigFile, SplitFeedbackDeltaSource,
};
pub use osd_template::{
    FinishLineCtx, OsdTemplateConfig, OsdTemplateConfigFile, OsdTemplatePreset, SectorLineCtx,
    SubSlotCtx,
};
pub use reference_times::{ReferenceTimesConfig, ReferenceTimesConfigFile};
pub use acr_timing_store::ReferenceTimeMode;
