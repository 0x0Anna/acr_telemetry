//! UTC calendar helpers shared by the exporters and the format metadata writer.

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};

/// Split Unix seconds into (year, month, day, hour, minute, second) in UTC.
pub fn utc_parts(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let dt = DateTime::<Utc>::from_timestamp(secs as i64, 0).unwrap_or_default();
    (dt.year() as u32, dt.month(), dt.day(), dt.hour(), dt.minute(), dt.second())
}

/// Format a timestamp as MoTeC header (`dd/mm/yyyy`, `HH:MM:SS`) in the timestamp's own zone.
fn motec_date_time<Tz: TimeZone>(dt: &DateTime<Tz>) -> (String, String) {
    (
        format!("{:02}/{:02}/{:04}", dt.day(), dt.month(), dt.year()),
        format!("{:02}:{:02}:{:02}", dt.hour(), dt.minute(), dt.second()),
    )
}

/// MoTeC header Log Date / Log Time: the recording start (RFC 3339, from `<stem>.notes.json`) in
/// local time, which is what MoTeC i2 and the logs it is aligned against use. Falls back to the
/// current local time when the start is missing or unparsable.
pub fn motec_log_date_time(recording_start_utc: Option<&str>) -> (String, String) {
    let start = recording_start_utc
        .and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok())
        .map(|dt| dt.with_timezone(&Local));
    motec_date_time(&start.unwrap_or_else(Local::now))
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

    #[test]
    fn header_uses_zone_of_the_timestamp() {
        use chrono::FixedOffset;
        let utc = DateTime::parse_from_rfc3339("2026-11-21T05:15:53.594852600+00:00").unwrap();
        assert_eq!(motec_date_time(&utc), ("21/11/2026".into(), "05:15:53".into()));
        // The same instant in CDT (UTC-5) is still the evening of the 20th.
        let cdt = utc.with_timezone(&FixedOffset::west_opt(5 * 3600).unwrap());
        assert_eq!(motec_date_time(&cdt), ("21/11/2026".into(), "00:15:53".into()));
        let cdt_prev = DateTime::parse_from_rfc3339("2026-11-21T02:19:02+00:00")
            .unwrap()
            .with_timezone(&FixedOffset::west_opt(5 * 3600).unwrap());
        assert_eq!(motec_date_time(&cdt_prev), ("20/11/2026".into(), "21:19:02".into()));
    }

    #[test]
    fn unparsable_start_falls_back_to_now() {
        let (d, t) = motec_log_date_time(Some("not a time"));
        assert_eq!(d.len(), 10);
        assert_eq!(t.len(), 8);
    }
}
