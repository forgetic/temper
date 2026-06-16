// SPDX-License-Identifier: MPL-2.0

//! Human duration formatting for the journal text (§6 / §7 of the design).
//!
//! The machine fields stay numeric — `duration_ms = 73000`, never `"1m13s"`
//! (§3 rule 4). Only the *human projection* formats a compact duration so the
//! operator reads `done in 1m13s` while an agent thresholds on the integer.
//!
//! [`format_duration_ms`] is the single renderer for that compact form. The
//! style matches §7's reference output: `45s`, `1m13s`, `4m37s`, `15m33s`, and
//! `1h02m03s` once an hour is reached. There is no leading zero on the most
//! significant unit, and lower units are zero-padded to two digits so the form
//! stays fixed-width within a tier (`1m05s`, not `1m5s`).

use std::time::Duration;

/// Formats a millisecond count as a compact human duration (`1m13s`).
///
/// Rules:
/// - Sub-minute renders just seconds: `0s`, `9s`, `45s`.
/// - A minute or more renders `<m>m<ss>s` with seconds zero-padded: `1m05s`,
///   `15m33s`.
/// - An hour or more prepends `<h>h<mm>m<ss>s`: `1h02m03s`.
/// - The most significant unit is never zero-padded; lower units always are.
///
/// Milliseconds below a whole second are truncated (floored), matching "elapsed
/// whole seconds" semantics — `1999 ms` is `1s`, not `2s`.
pub fn format_duration_ms(duration_ms: u64) -> String {
    let total_seconds = duration_ms / 1_000;
    let seconds = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    let minutes = total_minutes % 60;
    let hours = total_minutes / 60;

    if hours > 0 {
        format!("{hours}h{minutes:02}m{seconds:02}s")
    } else if total_minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

/// Formats a [`Duration`] as a compact human duration (`1m13s`).
///
/// Convenience wrapper over [`format_duration_ms`]; sub-millisecond precision is
/// not represented in the compact form. Durations longer than `u64::MAX` ms
/// saturate (not reachable for any real elapsed wall-clock span).
pub fn format_duration(duration: Duration) -> String {
    let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    format_duration_ms(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_only_below_a_minute() {
        assert_eq!(format_duration_ms(0), "0s");
        assert_eq!(format_duration_ms(999), "0s"); // sub-second truncates
        assert_eq!(format_duration_ms(1_000), "1s");
        assert_eq!(format_duration_ms(9_000), "9s");
        assert_eq!(format_duration_ms(45_000), "45s");
        assert_eq!(format_duration_ms(59_000), "59s");
        assert_eq!(format_duration_ms(59_999), "59s"); // floor, not round
    }

    #[test]
    fn minute_boundary_zero_pads_seconds() {
        assert_eq!(format_duration_ms(60_000), "1m00s");
        assert_eq!(format_duration_ms(65_000), "1m05s");
        // The exact §7 examples.
        assert_eq!(format_duration_ms(73_000), "1m13s");
        assert_eq!(format_duration_ms(191_000), "3m11s");
        assert_eq!(format_duration_ms(277_000), "4m37s");
        assert_eq!(format_duration_ms(559_000), "9m19s");
        assert_eq!(format_duration_ms(933_000), "15m33s");
    }

    #[test]
    fn ten_plus_minutes_keep_two_digit_seconds() {
        assert_eq!(format_duration_ms(744_000), "12m24s");
        assert_eq!(format_duration_ms(600_000), "10m00s");
    }

    #[test]
    fn hour_boundary_renders_three_tiers() {
        assert_eq!(format_duration_ms(3_600_000), "1h00m00s");
        // 1h 02m 03s = 3600 + 123 = 3723 s.
        assert_eq!(format_duration_ms(3_723_000), "1h02m03s");
        // 2h 30m 00s.
        assert_eq!(format_duration_ms(9_000_000), "2h30m00s");
    }

    #[test]
    fn duration_wrapper_matches_ms_form() {
        assert_eq!(format_duration(Duration::from_secs(73)), "1m13s");
        assert_eq!(format_duration(Duration::from_millis(45_500)), "45s");
        assert_eq!(format_duration(Duration::from_secs(3_723)), "1h02m03s");
    }
}
