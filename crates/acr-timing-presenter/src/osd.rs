//! Sector line formatting: `S1: +0.423 [0:19.34] [--] ref: 1:31.45 tot: 0:45.32`
//! With RTSS: `+`/brackets colored red (slower) / green (faster) via `<C=RRGGBB>`.

use acr_timing::rtss_osd::hypertext;

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
) -> String {
    let prefix = if incomplete_mark {
        format!("S{}~:", sector_index + 1)
    } else {
        format!("S{}:", sector_index + 1)
    };
    let sign = if cum_delta_sec >= 0.0 { "+" } else { "" };
    let cum_text = format!("{sign}{cum_delta_sec:.3}");
    let cum_colored = if rtss_colors {
        hypertext::wrap_delta_colored(cum_delta_sec, &cum_text)
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
                .map(|d| hypertext::wrap_delta_colored(d, &bracket))
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

/// After Finish: sum of sector `tot` / reference totals and Σ sector cum Δ.
pub fn format_track_completed_line(
    cum_tot_sec: f64,
    cum_ref_tot_sec: f64,
    cum_delta_sec: f64,
    rtss_colors: bool,
) -> String {
    let mut parts = vec![
        "Track completed".to_string(),
        format!("cum: {}", format_duration(cum_tot_sec)),
    ];
    if cum_ref_tot_sec.is_finite() && cum_ref_tot_sec >= 0.0 {
        parts.push(format!("ref: {}", format_duration(cum_ref_tot_sec)));
    }
    let sign = if cum_delta_sec >= 0.0 { "+" } else { "" };
    let delta_body = format!("{sign}{cum_delta_sec:.3}");
    let delta = if rtss_colors && cum_delta_sec.is_finite() && cum_delta_sec.abs() > 1e-9 {
        format!(
            "delta: {}",
            hypertext::wrap_delta_colored(cum_delta_sec, &delta_body)
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

    #[test]
    fn sector_line_with_gap() {
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
        );
        assert!(line.contains("+0.500"));
        assert!(line.contains("[--]"));
        assert!(line.contains("ref: 1:31.45"));
        assert!(line.contains("tot:"));
    }

    #[test]
    fn track_completed_line() {
        let line = format_track_completed_line(272.5, 270.0, 2.5, true);
        assert!(line.contains("Track completed"));
        assert!(line.contains("cum:"));
        assert!(line.contains("delta:"));
        assert!(line.contains("<C=ff0000>"));
    }
}
