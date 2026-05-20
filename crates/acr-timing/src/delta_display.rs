//! Δ display: split-feedback source and RTSS colors (neutral zone, colorblind-friendly).

use crate::rtss_osd::hypertext;
use serde::Deserialize;

/// Which Δ drives cumulative split WAV/beeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitFeedbackDeltaSource {
    /// Per subsector / gate: `delta_i` (fallback: cumulative Δ in sector).
    #[default]
    Subsector,
    /// Main-sector cumulative Δ (`cum_delta` within the current sector block).
    Sector,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeltaDisplayConfigFile {
    /// `subsector` | `sector` (aliases: `stage`, `delta_i`, `cum`, `cum_delta`, `tot`).
    #[serde(default = "default_split_feedback")]
    pub split_feedback: String,
    #[serde(default = "default_neutral_zone")]
    pub neutral_zone_sec: f64,
    /// RTSS `<C=RRGGBB>` for negative Δ (faster). No `#` prefix.
    #[serde(default = "default_faster_color")]
    pub faster_color: String,
    #[serde(default = "default_slower_color")]
    pub slower_color: String,
}

fn default_split_feedback() -> String {
    "subsector".into()
}
fn default_neutral_zone() -> f64 {
    0.05
}
fn default_faster_color() -> String {
    "00ff00".into()
}
fn default_slower_color() -> String {
    "ff0000".into()
}

impl Default for DeltaDisplayConfigFile {
    fn default() -> Self {
        Self {
            split_feedback: default_split_feedback(),
            neutral_zone_sec: default_neutral_zone(),
            faster_color: default_faster_color(),
            slower_color: default_slower_color(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeltaDisplayConfig {
    pub split_feedback: SplitFeedbackDeltaSource,
    pub colors: DeltaColorStyle,
}

#[derive(Debug, Clone)]
pub struct DeltaColorStyle {
    pub neutral_zone_sec: f64,
    pub faster_color_rgb: String,
    pub slower_color_rgb: String,
}

impl Default for DeltaColorStyle {
    fn default() -> Self {
        DeltaDisplayConfigFile::default().colors()
    }
}

impl DeltaColorStyle {
    pub fn wrap_delta(&self, delta: f64, text: &str) -> String {
        hypertext::wrap_delta_colored_styled(
            delta,
            text,
            self.neutral_zone_sec,
            &self.slower_color_rgb,
            &self.faster_color_rgb,
        )
    }
}

impl DeltaDisplayConfigFile {
    pub fn to_runtime(&self) -> DeltaDisplayConfig {
        DeltaDisplayConfig {
            split_feedback: parse_split_feedback(&self.split_feedback),
            colors: self.colors(),
        }
    }

    pub fn colors(&self) -> DeltaColorStyle {
        DeltaColorStyle {
            neutral_zone_sec: self.neutral_zone_sec.max(0.0),
            faster_color_rgb: normalize_rgb(&self.faster_color),
            slower_color_rgb: normalize_rgb(&self.slower_color),
        }
    }
}

impl Default for DeltaDisplayConfig {
    fn default() -> Self {
        DeltaDisplayConfigFile::default().to_runtime()
    }
}

fn normalize_rgb(s: &str) -> String {
    s.trim()
        .trim_start_matches('#')
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(6)
        .collect()
}

fn parse_split_feedback(s: &str) -> SplitFeedbackDeltaSource {
    match s.trim().to_ascii_lowercase().as_str() {
        "sector" | "stage" | "cum" | "cum_delta" | "tot" | "delta_tot" | "cumulative" => {
            SplitFeedbackDeltaSource::Sector
        }
        _ => SplitFeedbackDeltaSource::Subsector,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_split_feedback_aliases() {
        assert_eq!(
            parse_split_feedback("sector"),
            SplitFeedbackDeltaSource::Sector
        );
        assert_eq!(
            parse_split_feedback("stage"),
            SplitFeedbackDeltaSource::Sector
        );
        assert_eq!(
            parse_split_feedback("delta_i"),
            SplitFeedbackDeltaSource::Subsector
        );
    }
}
