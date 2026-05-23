//! Three-line RTSS layout for `[osd_display] preset = "minimal"`.
//!
//! Pre-lock: `Game Data available` when JSONL is fresh (optional single line).
//! Pre-start: `ref: [t1] [t2] … tot: [sum]` | big `0` | `Timer ready` / `Timing ready`
//! In-run: `[time ±Δ]` … | big Δ | status empty (flashes elsewhere)

use std::path::Path;
use std::time::Instant;

use crate::delta_display::DeltaColorStyle;
use crate::game_clock_sync::{read_latest_sample, GameClockSample};
use crate::osd_template::wrap_rtss_font_scale;
use crate::stage_sector_timing::{
    format_duration, reference_sector_secs_from_pb, StageSectorSession,
};
use crate::timing_pb::TimingPbStore;

const PRESTART_RACE_TIME_MAX_SEC: f64 = 3.0;

/// Upper-line placeholder for one sector slot while paused.
pub const MINIMAL_PAUSE_SECTOR_SLOT: &str = "[--]";

/// All sector slots show `--` (pause / game time frozen).
pub fn format_minimal_pause_sector_tape(sector_count: usize) -> String {
    let n = sector_count.max(1);
    std::iter::repeat_n(MINIMAL_PAUSE_SECTOR_SLOT, n)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn ready_status_text(timer_ready: bool) -> &'static str {
    if timer_ready {
        "Timer ready"
    } else {
        "Timing ready"
    }
}

/// RTSS status before track-lock when UE4SS JSONL is fresh.
pub const GAME_DATA_AVAILABLE_TEXT: &str = "Game Data available";

pub fn game_clock_timer_ready(path: &Path, max_age_sec: f64) -> bool {
    read_latest_sample(path, max_age_sec).is_some()
}

/// Single-line RTSS before track-lock (see `TIMING_OPERATING_MODES.md` §3.1).
pub fn compose_minimal_pre_lock_osd(game_data_available: bool) -> String {
    if game_data_available {
        GAME_DATA_AVAILABLE_TEXT.to_string()
    } else {
        String::new()
    }
}

/// Pre-start upper line: `ref: [1:31.45] … tot: [9:12.34]`.
pub fn format_minimal_pre_start_reference_line(refs: &[Option<f64>]) -> String {
    let brackets = format_minimal_reference_tape(refs);
    if brackets.is_empty() {
        return String::new();
    }
    let tot: f64 = refs
        .iter()
        .filter_map(|r| *r)
        .filter(|t| t.is_finite() && *t >= 0.0)
        .sum();
    let tot_part = if tot >= 0.05 {
        format!(" tot: [{}]", format_duration(tot))
    } else {
        String::new()
    };
    format!("ref: {brackets}{tot_part}")
}

/// Pre-start middle line: cumulative Δ fixed at zero.
pub fn format_minimal_pre_start_big_delta(
    rtss: bool,
    delta_style: &DeltaColorStyle,
    font_scale: u32,
) -> String {
    format_minimal_big_delta(0.0, rtss, delta_style, font_scale)
}

pub fn penalty_from_sample(sample: &GameClockSample) -> Option<f64> {
    sample
        .penalty_total_s
        .or_else(|| sample.ghost_ref.as_ref().and_then(|g| g.penalty_total_s))
        .filter(|p| p.is_finite() && *p > 0.0)
}

fn format_delta_compact(delta_sec: f64) -> String {
    if !delta_sec.is_finite() {
        return "...".to_string();
    }
    if delta_sec >= 0.0 {
        format!("+{:.2}", delta_sec)
    } else {
        format!("{:.2}", delta_sec)
    }
}

/// One sector slot: `[1:23.34 +1.23]`.
pub fn format_minimal_sector_bracket(
    tot_sec: f64,
    delta_sec: f64,
    rtss: bool,
    delta_style: &DeltaColorStyle,
) -> String {
    let time = format_duration(tot_sec);
    let dtext = format_delta_compact(delta_sec);
    let inner = if rtss && delta_sec.is_finite() {
        format!("{} {}", time, delta_style.wrap_delta(delta_sec, &dtext))
    } else {
        format!("{time} {dtext}")
    };
    format!("[{inner}]")
}

/// Pre-start: reference times only, e.g. `[1:31.45] [1:45.22] [2:01.33] [4:12.50]`.
pub fn format_minimal_reference_tape(refs: &[Option<f64>]) -> String {
    refs.iter()
        .filter_map(|r| {
            r.filter(|t| t.is_finite() && *t >= 0.0)
                .map(|t| format!("[{}]", format_duration(t)))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn effective_cur_sec(
    leg_i: usize,
    current_secs: &[Option<f64>],
    highlight_leg: Option<usize>,
    live_elapsed_sec: Option<f64>,
) -> Option<f64> {
    if highlight_leg == Some(leg_i) {
        return live_elapsed_sec.or_else(|| current_secs.get(leg_i).copied().flatten());
    }
    current_secs.get(leg_i).copied().flatten()
}

/// In-run sector tape (completed legs + live highlighted leg).
pub fn format_minimal_run_tape(
    refs: &[Option<f64>],
    current_secs: &[Option<f64>],
    highlight_leg: Option<usize>,
    live_elapsed_sec: Option<f64>,
    rtss: bool,
    delta_style: &DeltaColorStyle,
) -> String {
    let n = current_secs.len();
    let refs = align_refs(refs, n);
    let mut parts = Vec::new();
    for i in 0..n {
        // Live slot only while that leg has no stored time yet (avoids duplicate bracket).
        let is_live_leg =
            highlight_leg == Some(i) && current_secs.get(i).copied().flatten().is_none();
        let cur = if is_live_leg {
            effective_cur_sec(i, current_secs, highlight_leg, live_elapsed_sec)
        } else {
            current_secs.get(i).copied().flatten()
        };
        let Some(cur) = cur else {
            continue;
        };
        if !cur.is_finite() {
            continue;
        }
        if is_live_leg {
            parts.push(format!("[{}]", format_duration(cur)));
            continue;
        }
        let delta = refs
            .get(i)
            .copied()
            .flatten()
            .filter(|r| r.is_finite() && *r >= 0.0)
            .map(|r| cur - r)
            .filter(|d| d.is_finite());
        if let Some(d) = delta {
            parts.push(format_minimal_sector_bracket(cur, d, rtss, delta_style));
        } else {
            parts.push(format!("[{}]", format_duration(cur)));
        }
    }
    parts.join(" ")
}

fn align_refs(refs: &[Option<f64>], n: usize) -> Vec<Option<f64>> {
    let mut v: Vec<Option<f64>> = refs.iter().copied().collect();
    v.truncate(n);
    while v.len() < n {
        v.push(None);
    }
    v
}

/// Large cumulative Δ (second OSD line).
pub fn format_minimal_big_delta(
    delta_sec: f64,
    rtss: bool,
    delta_style: &DeltaColorStyle,
    font_scale: u32,
) -> String {
    let sign = if delta_sec >= 0.0 { "+" } else { "" };
    let text = format!("{sign}{delta_sec:.3}");
    let colored = if rtss {
        delta_style.wrap_delta(delta_sec, &text)
    } else {
        text
    };
    wrap_rtss_font_scale(&colored, font_scale, rtss)
}

/// Like [`format_minimal_big_delta`]; missing/invalid Δ shows `--` (no bogus stage sum).
pub fn format_minimal_big_delta_opt(
    delta_sec: Option<f64>,
    rtss: bool,
    delta_style: &DeltaColorStyle,
    font_scale: u32,
) -> String {
    match delta_sec.filter(|d| d.is_finite()) {
        Some(d) => format_minimal_big_delta(d, rtss, delta_style, font_scale),
        None => wrap_rtss_font_scale("--", font_scale, rtss),
    }
}

/// Penalty suffix for the upper line (red when RTSS).
pub fn format_minimal_penalty_suffix(penalty_sec: f64, rtss: bool) -> String {
    if !penalty_sec.is_finite() || penalty_sec <= 0.0 {
        return String::new();
    }
    let text = format!("P {}", format_duration(penalty_sec));
    if rtss {
        format!("  <C=ff0000>{text}<C>")
    } else {
        format!("  {text}")
    }
}

pub fn stage_sessions_pre_start(sessions: &[&StageSectorSession]) -> bool {
    sessions.iter().all(|s| !s.run.armed && !s.run.completed)
}

pub fn pre_start_from_race_time(race_time_s: Option<f64>) -> bool {
    match race_time_s.filter(|t| t.is_finite()) {
        Some(t) => t <= PRESTART_RACE_TIME_MAX_SEC,
        None => true,
    }
}

/// Upper line for one stage session (refs or live tape).
pub fn format_minimal_stage_upper(
    session: &StageSectorSession,
    pb: &TimingPbStore,
    car_model: &str,
    now: Instant,
    pre_start: bool,
    pause_dash: bool,
    rtss: bool,
    delta_style: &DeltaColorStyle,
    game_race_s: Option<f64>,
) -> String {
    let car = car_model.trim();
    let car = if car.is_empty() { "unknown_car" } else { car };
    let refs = reference_sector_secs_from_pb(
        pb,
        &session.markers.stage_slug,
        car,
        &session.markers.markers,
    );
    if pre_start {
        format_minimal_pre_start_reference_line(&refs)
    } else if pause_dash {
        format_minimal_pause_sector_tape(session.run.sector_secs.len())
    } else {
        format_minimal_run_tape(
            &refs,
            &session.run.sector_secs,
            session.run.highlight_leg_index(),
            session.run.live_leg_elapsed_sec(now, game_race_s),
            rtss,
            delta_style,
        )
    }
}

pub fn format_minimal_multi_stage_upper(
    sessions: &[&StageSectorSession],
    pb: &TimingPbStore,
    car_model: &str,
    now: Instant,
    pre_start: bool,
    pause_dash: bool,
    rtss: bool,
    delta_style: &DeltaColorStyle,
    game_race_s: Option<f64>,
) -> String {
    let sep = if rtss { " || " } else { "  ‖  " };
    sessions
        .iter()
        .take(crate::stage_timing_config::MAX_PARALLEL_STAGE_TIMINGS)
        .map(|sess| {
            format_minimal_stage_upper(
                sess, pb, car_model, now, pre_start, pause_dash, rtss, delta_style, game_race_s,
            )
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(sep)
}

pub fn compose_minimal_timing_osd(upper: &str, delta_line: &str, status: &str) -> String {
    let mut lines = Vec::new();
    let upper = upper.trim();
    let delta_line = delta_line.trim();
    let status = status.trim();
    if !upper.is_empty() {
        lines.push(upper.to_string());
    }
    if !delta_line.is_empty() {
        lines.push(delta_line.to_string());
    }
    if !status.is_empty() {
        lines.push(status.to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta_display::DeltaColorStyle;

    #[test]
    fn reference_tape_formats_brackets() {
        let refs = vec![Some(91.5), Some(102.0), None];
        let s = format_minimal_reference_tape(&refs);
        assert!(s.contains('['));
        assert!(s.contains("1:31"));
    }

    #[test]
    fn ready_status_strings() {
        assert_eq!(ready_status_text(true), "Timer ready");
        assert_eq!(ready_status_text(false), "Timing ready");
    }

    #[test]
    fn pre_start_reference_line_has_ref_prefix_and_tot() {
        let refs = vec![Some(91.5), Some(102.0), Some(61.0), Some(120.0)];
        let s = format_minimal_pre_start_reference_line(&refs);
        assert!(s.starts_with("ref: "));
        assert!(s.contains("tot: ["));
        assert!(s.contains("6:14")); // 91.5+102+61+120 = 374.5s = 6:14.50
    }

    #[test]
    fn pre_lock_osd_only_when_data_available() {
        assert_eq!(
            compose_minimal_pre_lock_osd(true),
            GAME_DATA_AVAILABLE_TEXT
        );
        assert!(compose_minimal_pre_lock_osd(false).is_empty());
    }
}
