//! Small host-side utilities (UTC timestamps without a date/time dependency).

use std::time::{SystemTime, UNIX_EPOCH};

/// Converts days since 1970-01-01 into a civil (year, month, day) triple.
/// Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (
        if month <= 2 { year + 1 } else { year },
        month,
        day,
    )
}

/// Formats a UNIX timestamp (seconds, UTC) as `YYYY-MM-DD_HH-MM-SS`.
pub fn format_utc_timestamp(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}_{hour:02}-{minute:02}-{second:02}")
}

/// Current UTC timestamp for file names and reports.
pub fn timestamp_string() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_utc_timestamp(seconds)
}

/// Formats a nanosecond duration compactly.
pub fn format_ns(ns: u64) -> String {
    if ns < 1_000 {
        format!("{ns} ns")
    } else if ns < 1_000_000 {
        format!("{:.2} us", ns as f64 / 1e3)
    } else if ns < 1_000_000_000 {
        format!("{:.2} ms", ns as f64 / 1e6)
    } else {
        format!("{:.3} s", ns as f64 / 1e9)
    }
}

pub fn format_seconds(seconds: f64) -> String {
    if seconds < 1.0 {
        format!("{:.1} ms", seconds * 1000.0)
    } else {
        format!("{:.3} s", seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_known_timestamps() {
        assert_eq!(format_utc_timestamp(0), "1970-01-01_00-00-00");
        assert_eq!(format_utc_timestamp(1_700_000_000), "2023-11-14_22-13-20");
        assert_eq!(format_utc_timestamp(1_234_567_890), "2009-02-13_23-31-30");
        // Leap day.
        assert_eq!(format_utc_timestamp(1_582_934_400), "2020-02-29_00-00-00");
    }
}
