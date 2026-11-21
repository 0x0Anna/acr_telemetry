//! UTC calendar helpers shared by the exporters and the format metadata writer.

use chrono::{DateTime, Datelike, Timelike, Utc};

/// Split Unix seconds into (year, month, day, hour, minute, second) in UTC.
pub fn utc_parts(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let dt = DateTime::<Utc>::from_timestamp(secs as i64, 0).unwrap_or_default();
    (dt.year() as u32, dt.month(), dt.day(), dt.hour(), dt.minute(), dt.second())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch() {
        assert_eq!(utc_parts(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn leap_day() {
        // 2024-02-29T12:34:56Z
        assert_eq!(utc_parts(1_709_210_096), (2024, 2, 29, 12, 34, 56));
    }

    #[test]
    fn known_recordings() {
        assert_eq!(utc_parts(1_792_030_742), (2026, 10, 15, 2, 19, 2));
        assert_eq!(utc_parts(1_785_946_105), (2026, 8, 5, 16, 8, 25));
    }
}
