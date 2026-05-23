//! Sector line formatting: `S1: +0.423 [0:19.34] [--] ref: 1:31.45 tot: 0:45.32`
//! With RTSS: `+`/brackets colored red (slower) / green (faster) via `<C=RRGGBB>`.
//! Upper OSD tape: `[1:32.32 -0.12] [1:21.25 +0.10]` (sector time + sector Δ).

use acr_timing::delta_display::DeltaColorStyle;
use acr_timing::osd_template::{
    format_finish_line_templated, format_live_sector_line_templated,
    format_sector_line_templated, wrap_rtss_font_scale, FinishLineCtx, OsdTemplateConfig,
    SectorLineCtx, SubSlotCtx,
};
use acr_timing_protocol::SectorCompleted;

const MAX_SUB_SLOTS: usize = 8;

pub fn format_duration(sec: f64) -> String {
    if !sec.is_finite() || sec < 0.0 {
        return "--:--.--".to_string();
    }
    let total_cs = (sec * 100.0).round() as u64;
    let cs = total_cs % 100;
    let total_s = total_cs / 100;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{cs:02}")
    } else {
        format!("{m}:{s:02}.{cs:02}")
    }
}

/// Compact signed Δ for sector tape, e.g. `-0.12` / `+0.10`.
pub fn format_delta_compact(delta_sec: f64) -> String {
    if !delta_sec.is_finite() {
        return "...".to_string();
    }
    if delta_sec >= 0.0 {
        format!("+{:.2}", delta_sec)
    } else {
        format!("{:.2}", delta_sec)
    }
}

/// One completed sector: `[1:32.32 -0.12]`.
pub fn format_sector_bracket(
    tot_sec: f64,
    delta_sec: f64,
    rtss_colors: bool,
    delta_style: &DeltaColorStyle,
) -> String {
    let time = format_duration(tot_sec);
    let dtext = format_delta_compact(delta_sec);
    let inner = if rtss_colors && delta_sec.is_finite() {
        format!(
            "{} {}",
            time,
            delta_style.wrap_delta(delta_sec, &dtext)
        )
    } else {
        format!("{time} {dtext}")
    };
    format!("[{inner}]")
}

/// All completed sectors on the upper OSD line.
pub fn format_sector_tape(
    completed: &[SectorCompleted],
    rtss_colors: bool,
    delta_style: &DeltaColorStyle,
) -> String {
    completed
        .iter()
        .map(|s| format_sector_bracket(s.tot_sec, s.cum_delta_sec, rtss_colors, delta_style))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reference times for the upcoming sector (only before the sector timer runs).
pub fn format_sector_reference_line(
    sector_index: u32,
    reference_tot_sec: Option<f64>,
    reference_sub_times_sec: &[f64],
) -> String {
    let label = format!("S{} ref", sector_index + 1);
    if let Some(t) = reference_tot_sec.filter(|t| t.is_finite() && *t >= 0.0) {
        return format!("{label} {}", format_duration(t));
    }
    let refs: Vec<String> = reference_sub_times_sec
        .iter()
        .filter(|t| t.is_finite() && **t >= 0.0)
        .map(|t| format_duration(*t))
        .collect();
    if refs.is_empty() {
        label
    } else {
        format!("{label} {}", refs.join(" "))
    }
}

pub fn format_sector_line(
    sector_index: u32,
    cum_delta_sec: f64,
    sub_ids: &[i32],
    sub_times_sec: &[Option<f64>],
    sub_delta_sec: &[Option<f64>],
    reference_tot_sec: Option<f64>,
    tot_sec: f64,
    incomplete_mark: bool,
    rtss_colors: bool,
    delta_style: &DeltaColorStyle,
) -> String {
    let prefix = if incomplete_mark {
        format!("S{}~:", sector_index + 1)
    } else {
        format!("S{}:", sector_index + 1)
    };
    let sign = if cum_delta_sec >= 0.0 { "+" } else { "" };
    let cum_text = format!("{sign}{cum_delta_sec:.3}");
    let cum_colored = if rtss_colors {
        delta_style.wrap_delta(cum_delta_sec, &cum_text)
    } else {
        cum_text
    };
    let mut parts = vec![format!("{prefix} {cum_colored}")];

    let n = sub_ids.len();
    let start = n.saturating_sub(MAX_SUB_SLOTS);
    for i in start..n {
        let slot = sub_times_sec
            .get(i)
            .and_then(|t| *t)
            .filter(|t| t.is_finite())
            .map(format_duration)
            .unwrap_or_else(|| "--".to_string());
        let bracket = format!("[{slot}]");
        let delta = sub_delta_sec.get(i).copied().flatten();
        let part = if rtss_colors {
            delta
                .filter(|d| d.is_finite())
                .map(|d| delta_style.wrap_delta(d, &bracket))
                .unwrap_or(bracket)
        } else {
            bracket
        };
        parts.push(part);
    }

    if let Some(ref_sec) = reference_tot_sec.filter(|t| t.is_finite() && *t >= 0.0) {
        parts.push(format!("ref: {}", format_duration(ref_sec)));
    }
    parts.push(format!("tot: {}", format_duration(tot_sec)));
    parts.join(" ")
}

/// Active sector while driving (`live_sector_line`; optional RTSS `<S=…>` on live Δ only).
pub fn format_live_sector_line(
    sector_index: u32,
    cum_delta_sec: f64,
    sub_ids: &[i32],
    sub_times_sec: &[Option<f64>],
    sub_delta_sec: &[Option<f64>],
    reference_tot_sec: Option<f64>,
    tot_sec: f64,
    rtss_colors: bool,
    delta_style: &DeltaColorStyle,
    templates: Option<&OsdTemplateConfig>,
) -> String {
    let line = if let Some(tpl) = templates {
        let subs: Vec<SubSlotCtx> = sub_ids
            .iter()
            .enumerate()
            .map(|(i, _)| SubSlotCtx {
                time_sec: sub_times_sec.get(i).copied().flatten(),
                delta_sec: sub_delta_sec.get(i).copied().flatten(),
            })
            .collect();
        format_live_sector_line_templated(
            tpl,
            &SectorLineCtx {
                sector_index,
                cum_delta_sec,
                tot_sec,
                reference_tot_sec,
                incomplete: false,
                subs,
            },
            rtss_colors,
            delta_style,
        )
    } else {
        format_sector_line(
            sector_index,
            cum_delta_sec,
            sub_ids,
            sub_times_sec,
            sub_delta_sec,
            reference_tot_sec,
            tot_sec,
            false,
            rtss_colors,
            delta_style,
        )
    };
    let scale = templates.map(|t| t.live_delta_font_scale).unwrap_or(0);
    wrap_rtss_font_scale(&line, scale, rtss_colors)
}

/// One sector for post-finish carousel (`sector_line` template, normal font).
pub fn format_carousel_sector_line(
    s: &SectorCompleted,
    rtss_colors: bool,
    delta_style: &DeltaColorStyle,
    templates: Option<&OsdTemplateConfig>,
) -> String {
    if let Some(tpl) = templates {
        let subs: Vec<SubSlotCtx> = s
            .sub_ids
            .iter()
            .enumerate()
            .map(|(i, _)| SubSlotCtx {
                time_sec: s.sub_times_sec.get(i).copied().flatten(),
                delta_sec: s.sub_delta_sec.get(i).copied().flatten(),
            })
            .collect();
        return format_sector_line_templated(
            tpl,
            &SectorLineCtx {
                sector_index: s.sector_index,
                cum_delta_sec: s.cum_delta_sec,
                tot_sec: s.tot_sec,
                reference_tot_sec: s.reference_tot_sec.is_finite().then_some(s.reference_tot_sec),
                incomplete: false,
                subs,
            },
            rtss_colors,
            delta_style,
        );
    }
    format_sector_bracket(s.tot_sec, s.cum_delta_sec, rtss_colors, delta_style)
}

/// After Finish: sum of sector `tot` / reference totals and Σ sector cum Δ (no enlarged font).
pub fn format_track_completed_line(
    cum_tot_sec: f64,
    cum_ref_tot_sec: f64,
    cum_delta_sec: f64,
    rtss_colors: bool,
    delta_style: &DeltaColorStyle,
    templates: Option<&OsdTemplateConfig>,
) -> String {
    if let Some(tpl) = templates {
        return format_finish_line_templated(
            tpl,
            &FinishLineCtx {
                cum_tot_sec,
                ref_tot_sec: cum_ref_tot_sec,
                cum_delta_sec,
            },
            rtss_colors,
            delta_style,
        );
    }
    let mut parts = vec![
        "Track completed".to_string(),
        format!("cum: {}", format_duration(cum_tot_sec)),
    ];
    if cum_ref_tot_sec.is_finite() && cum_ref_tot_sec >= 0.0 {
        parts.push(format!("ref: {}", format_duration(cum_ref_tot_sec)));
    }
    let sign = if cum_delta_sec >= 0.0 { "+" } else { "" };
    let delta_body = format!("{sign}{cum_delta_sec:.3}");
    let delta = if rtss_colors && cum_delta_sec.is_finite() {
        format!(
            "delta: {}",
            delta_style.wrap_delta(cum_delta_sec, &delta_body)
        )
    } else {
        format!("delta: {delta_body}")
    };
    parts.push(delta);
    parts.join("  ")
}

pub fn compose_osd_message(status: &str, presenter_lines: &[String]) -> String {
    let mut lines = vec![status.to_string()];
    lines.extend(presenter_lines.iter().cloned());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use acr_timing::delta_display::DeltaColorStyle;

    #[test]
    fn sector_line_with_gap() {
        let style = DeltaColorStyle::default();
        let line = format_sector_line(
            0,
            0.5,
            &[1, 2, 3],
            &[Some(21.5), None, Some(14.0)],
            &[Some(0.5), None, Some(-1.0)],
            Some(91.45),
            35.5,
            false,
            false,
            &style,
        );
        assert!(line.contains("+0.500"));
        assert!(line.contains("[--]"));
        assert!(line.contains("ref: 1:31.45"));
        assert!(line.contains("tot:"));
    }

    #[test]
    fn track_completed_line() {
        let style = DeltaColorStyle::default();
        let line = format_track_completed_line(272.5, 270.0, 2.5, true, &style, None);
        assert!(line.contains("Track completed"));
        assert!(line.contains("cum:"));
        assert!(line.contains("delta:"));
        assert!(line.contains("<C=ff0000>"));
    }

    #[test]
    fn neutral_zone_uses_default_color() {
        let style = DeltaColorStyle {
            neutral_zone_sec: 0.05,
            faster_color_rgb: "00aaff".into(),
            slower_color_rgb: "ff8800".into(),
        };
        let inside = style.wrap_delta(0.03, "+0.030");
        assert!(!inside.contains("<C="));
        let slow = style.wrap_delta(0.12, "+0.120");
        assert!(slow.contains("<C=ff8800>"));
        let fast = style.wrap_delta(-0.12, "-0.120");
        assert!(fast.contains("<C=00aaff>"));
    }
}
