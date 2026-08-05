//! Map cumulative gate ids to main sector indices using stage marker labels.

/// Build sector boundaries: subs between `Sector N` and `Sector N+1` gates.
pub fn sector_boundaries_from_labels(
    ordered: &[(i32, String)],
) -> Vec<super::sector_session::SectorBoundary> {
    let mut sector_ends: Vec<(u32, usize)> = Vec::new();
    for (i, (_, label)) in ordered.iter().enumerate() {
        if let Some(n) = parse_sector_end_label(label) {
            sector_ends.push((n, i));
        }
    }
    if sector_ends.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut prev_end_idx = 0usize;
    for (sector_n, end_idx) in &sector_ends {
        let sector_index = sector_n.saturating_sub(1);
        let mut sub_ids: Vec<i32> = ordered[prev_end_idx..*end_idx]
            .iter()
            .filter(|(_, label)| !is_main_or_start(label))
            .map(|(id, _)| *id)
            .collect();
        // Closing leg (last CP → sector end gate) uses the end marker's seg_id.
        if let Some((end_seg, _)) = ordered.get(*end_idx) {
            sub_ids.push(*end_seg);
        }
        // Always emit a block per main sector gate (S1→S2 may have zero subs — wall time only).
        out.push(super::sector_session::SectorBoundary {
            sector_index,
            sub_ids,
        });
        prev_end_idx = *end_idx + 1;
    }
    out
}

/// Sector number for boundary markers (`Sector 1` → 1); `Finish` → 4 (last sector index 3).
fn parse_sector_end_label(label: &str) -> Option<u32> {
    if label == "Finish" {
        return Some(4);
    }
    let rest = label.strip_prefix("Sector ")?.trim();
    rest.parse::<u32>().ok()
}

fn is_main_or_start(label: &str) -> bool {
    label == "Start" || label.starts_with("Sector ") || label == "Finish"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_subs_between_sector_markers() {
        let ordered = vec![
            (0, "Start".into()),
            (1, "CP 1".into()),
            (2, "CP 2".into()),
            (8, "Sector 1".into()),
            (10, "CP S2-1".into()),
            (11, "CP S2-2".into()),
            (9, "Sector 2".into()),
        ];
        let b = sector_boundaries_from_labels(&ordered);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].sub_ids, vec![1, 2, 8]);
        assert_eq!(b[1].sub_ids, vec![10, 11, 9]);
    }

    #[test]
    fn hafren_like_route_has_empty_s1_to_s2_block() {
        let ordered = vec![
            (0, "Start".into()),
            (1, "CP 1".into()),
            (8, "Sector 1".into()),
            (9, "Sector 2".into()),
            (10, "CP S2-8".into()),
            (25, "Sector 3".into()),
            (33, "Finish".into()),
        ];
        let b = sector_boundaries_from_labels(&ordered);
        assert_eq!(b.len(), 4);
        assert_eq!(b[1].sub_ids, vec![9]);
        assert_eq!(b[1].sector_index, 1);
    }
}
