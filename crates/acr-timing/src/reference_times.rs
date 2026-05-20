//! Reference time selection for split / sector Δ (`acr_timing.toml` `[reference_times]`).

use acr_timing_store::ReferenceTimeMode;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ReferenceTimesConfigFile {
    /// `best_sector` | `best_stage` | `best_subsector`
    #[serde(default = "default_reference_mode")]
    pub mode: String,
}

fn default_reference_mode() -> String {
    "best_sector".into()
}

#[derive(Debug, Clone, Copy)]
pub struct ReferenceTimesConfig {
    pub mode: ReferenceTimeMode,
}

impl Default for ReferenceTimesConfig {
    fn default() -> Self {
        ReferenceTimesConfigFile::default().to_runtime()
    }
}

impl Default for ReferenceTimesConfigFile {
    fn default() -> Self {
        Self {
            mode: default_reference_mode(),
        }
    }
}

impl ReferenceTimesConfigFile {
    pub fn to_runtime(&self) -> ReferenceTimesConfig {
        ReferenceTimesConfig {
            mode: ReferenceTimeMode::parse(&self.mode),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes() {
        assert_eq!(
            ReferenceTimeMode::parse("best_stage"),
            ReferenceTimeMode::BestStage
        );
        assert_eq!(
            ReferenceTimeMode::parse("BestSubsector"),
            ReferenceTimeMode::BestSubsector
        );
    }
}
