//! Multi-line overlay text for resolving ambiguous picks while stationary (RTSS / overlay file).
//!
//! - [`AmbiguousPacenoteOverlayState`] — pacenote stages (slug + reference track).
//! - [`TrackStartPickOverlayState`] — reference track names from `start_points.geojson` when several
//!   recorded starts lie within the prefilter radius.
//!
//! Line limits match [`OVERLAY_MAX_LINES`]; RTSS sanitization lives in [`crate::rtss_osd::sanitize_multiline_osd_text`].

use crate::pacenote_course::PacenoteStagePick;
use crate::win_picker_input::PacenotePickerKeyTracker;

/// Total lines for picker OSD (one hint row + [`CANDIDATE_ROWS`]).
pub const OVERLAY_MAX_LINES: usize = 4;
/// Candidate rows under the hint line.
pub const CANDIDATE_ROWS: usize = OVERLAY_MAX_LINES - 1;

/// Open picker: sorted candidates, selection index, global key-edge tracker.
pub struct AmbiguousPacenoteOverlayState {
    pub candidates: Vec<PacenoteStagePick>,
    pub index: usize,
    pub keys: PacenotePickerKeyTracker,
}

/// Build overlay text: at most [`OVERLAY_MAX_LINES`] lines; scrolls a window when `candidates.len() > CANDIDATE_ROWS`.
pub fn build_overlay_text(state: &AmbiguousPacenoteOverlayState) -> String {
    let n = state.candidates.len();
    let header = if n > CANDIDATE_ROWS {
        format!(
            "pacenote pick {}/{}  Ctrl+arrows  Ctrl+Enter",
            state.index + 1,
            n
        )
    } else {
        "pacenote pick: Ctrl+arrows  Ctrl+Enter".to_string()
    };
    let mut lines = vec![header];
    if n == 0 {
        return lines.join("\n");
    }
    let rows = CANDIDATE_ROWS.min(n);
    let start = if n <= CANDIDATE_ROWS {
        0usize
    } else {
        state
            .index
            .saturating_sub(1)
            .min(n.saturating_sub(CANDIDATE_ROWS))
    };
    for row in 0..rows {
        let i = start + row;
        if i >= n {
            break;
        }
        let c = &state.candidates[i];
        let mark = if i == state.index { ">" } else { " " };
        lines.push(format!(
            "{} {}  ({})",
            mark, c.slug, c.reference_track
        ));
    }
    debug_assert!(lines.len() <= OVERLAY_MAX_LINES);
    lines.join("\n")
}

/// Several reference tracks have a `start_points.geojson` anchor within radius — pick which layout to lock.
pub struct TrackStartPickOverlayState {
    pub track_names: Vec<String>,
    pub index: usize,
    pub keys: PacenotePickerKeyTracker,
}

/// Same line budget as pacenote picker; rows are reference track names only.
pub fn build_track_start_pick_overlay_text(state: &TrackStartPickOverlayState) -> String {
    let n = state.track_names.len();
    let header = if n > CANDIDATE_ROWS {
        format!(
            "track pick {}/{}  start_points  Ctrl+arrows  Ctrl+Enter",
            state.index + 1,
            n
        )
    } else {
        "track pick: start_points  Ctrl+arrows  Ctrl+Enter".to_string()
    };
    let mut lines = vec![header];
    if n == 0 {
        return lines.join("\n");
    }
    let rows = CANDIDATE_ROWS.min(n);
    let start = if n <= CANDIDATE_ROWS {
        0usize
    } else {
        state
            .index
            .saturating_sub(1)
            .min(n.saturating_sub(CANDIDATE_ROWS))
    };
    for row in 0..rows {
        let i = start + row;
        if i >= n {
            break;
        }
        let name = &state.track_names[i];
        let mark = if i == state.index { ">" } else { " " };
        lines.push(format!("{} {}", mark, name));
    }
    debug_assert!(lines.len() <= OVERLAY_MAX_LINES);
    lines.join("\n")
}
