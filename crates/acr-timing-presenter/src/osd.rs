//! Sector line formatting: `S1: +0.423 [0:19.34] [--] … tot: 1:18.59`

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
    tot_sec: f64,
    incomplete_mark: bool,
) -> String {
    let prefix = if incomplete_mark {
        format!("S{}~:", sector_index + 1)
    } else {
        format!("S{}:", sector_index + 1)
    };
    let sign = if cum_delta_sec >= 0.0 { "+" } else { "" };
    let mut parts = vec![format!("{prefix} {sign}{cum_delta_sec:.3}")];

    let n = sub_ids.len();
    let start = n.saturating_sub(MAX_SUB_SLOTS);
    for i in start..n {
        let slot = sub_times_sec
            .get(i)
            .and_then(|t| *t)
            .filter(|t| t.is_finite())
            .map(format_duration)
            .unwrap_or_else(|| "--".to_string());
        parts.push(format!("[{slot}]"));
    }

    parts.push(format!("tot: {}", format_duration(tot_sec)));
    parts.join(" ")
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
            35.5,
            false,
        );
        assert!(line.contains("+0.500"));
        assert!(line.contains("[--]"));
        assert!(line.contains("tot:"));
    }
}
