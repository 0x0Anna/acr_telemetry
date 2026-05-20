//! How reference (PB) times are chosen for Δ display.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReferenceTimeMode {
    /// Sum of per-main-sector bests (best S1 + best S2 + …).
    BestStage,
    /// Fastest complete run per main sector (`reference_runs` / sector history).
    #[default]
    BestSector,
    /// Per sub-gate minimum leg time (may mix subs from different runs).
    BestSubsector,
}

impl ReferenceTimeMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "best_stage" | "stage" | "gesamt" | "total" => Self::BestStage,
            "best_subsector" | "subsector" | "sub" | "gate" => Self::BestSubsector,
            _ => Self::BestSector,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::BestStage => "best_stage",
            Self::BestSector => "best_sector",
            Self::BestSubsector => "best_subsector",
        }
    }
}
