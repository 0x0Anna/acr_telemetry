//! Δ display: scope (stage / sector / subsector), split-feedback source, RTSS colors.

use crate::rtss_osd::hypertext;
use serde::Deserialize;

/// Which Δ is shown on RTSS and drives cumulative split WAV/beeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeltaScope {
    /// Last sub gate: `delta_i` (fallback: sector cumulative Δ in that gate).
    #[default]
    Subsector,
    /// Cumulative Δ within the current main sector (resets each sector).
    Sector,
    /// Cumulative Δ for the whole stage (sum of sector Δ, no reset per sector).
    Stage,
}

/// Alias for older code / docs.
pub type SplitFeedbackDeltaSource = DeltaScope;

#[derive(Debug, Clone, Deserialize)]
pub struct DeltaDisplayConfigFile {
    /// `subsector` | `sector` | `stage` (legacy key: `split_feedback`).
    #[serde(
        rename = "delta_scope",
        alias = "split_feedback",
        default = "default_delta_scope"
    )]
    pub delta_scope: String,
    #[serde(default = "default_neutral_zone")]
    pub neutral_zone_sec: f64,
    /// RTSS `<C=RRGGBB>` for negative Δ (faster). No `#` prefix.
    #[serde(default = "default_faster_color")]
    pub faster_color: String,
    #[serde(default = "default_slower_color")]
    pub slower_color: String,
    /// After Finish: cycle completed sectors on the detail OSD line (seconds each; 0 = last sector only).
    #[serde(default = "default_sector_recap_sec")]
    pub sector_recap_sec: f64,
}

fn default_delta_scope() -> String {
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
fn default_sector_recap_sec() -> f64 {
    5.0
}

impl Default for DeltaDisplayConfigFile {
    fn default() -> Self {
        Self {
            delta_scope: default_delta_scope(),
            neutral_zone_sec: default_neutral_zone(),
            faster_color: default_faster_color(),
            slower_color: default_slower_color(),
            sector_recap_sec: default_sector_recap_sec(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeltaDisplayConfig {
    pub delta_scope: DeltaScope,
    pub colors: DeltaColorStyle,
    pub sector_recap_sec: f64,
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
            delta_scope: parse_delta_scope(&self.delta_scope),
            colors: self.colors(),
            sector_recap_sec: self.sector_recap_sec.max(0.0),
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

impl DeltaDisplayConfig {
    /// Back-compat accessor (same as [`delta_scope`](Self::delta_scope)).
    pub fn split_feedback(&self) -> DeltaScope {
        self.delta_scope
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

pub fn parse_delta_scope(s: &str) -> DeltaScope {
    match s.trim().to_ascii_lowercase().as_str() {
        "sector" | "cum" | "cum_delta" | "main_sector" | "main" => DeltaScope::Sector,
        "stage" | "tot" | "delta_tot" | "cumulative" | "run" | "overall" => DeltaScope::Stage,
        "subsector" | "sub" | "gate" | "delta_i" | "leg" | "split" => DeltaScope::Subsector,
        _ => DeltaScope::Subsector,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_delta_scope_modes() {
        assert_eq!(parse_delta_scope("sector"), DeltaScope::Sector);
        assert_eq!(parse_delta_scope("stage"), DeltaScope::Stage);
        assert_eq!(parse_delta_scope("subsector"), DeltaScope::Subsector);
        assert_eq!(parse_delta_scope("delta_i"), DeltaScope::Subsector);
    }

    #[test]
    fn split_feedback_alias_deserializes() {
        let cfg: DeltaDisplayConfigFile =
            toml::from_str(r#"split_feedback = "stage""#).expect("toml");
        assert_eq!(cfg.to_runtime().delta_scope, DeltaScope::Stage);
    }
}
