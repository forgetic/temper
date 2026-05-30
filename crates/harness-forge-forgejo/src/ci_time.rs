// SPDX-License-Identifier: MPL-2.0
//! Timestamp parsing for Forgejo Actions payloads.
//!
//! Forgejo's Actions endpoints report timestamps inconsistently: some fields are
//! unix-seconds integers, others RFC3339 strings. These helpers accept either
//! form and normalize to the portable [`Timestamp`]. They live apart from the CI
//! adapter so the parsing logic and its tests stay focused.
use harness_forge::Timestamp;
use serde_json::Value;

/// Parse an optional JSON timestamp field, returning `None` when absent.
pub(crate) fn opt_timestamp(value: &Option<Value>) -> Option<Timestamp> {
    value.as_ref().and_then(parse_timestamp)
}

/// Parse a timestamp from either a unix-seconds integer or an RFC3339 string.
pub(crate) fn parse_timestamp(value: &Value) -> Option<Timestamp> {
    match value {
        Value::Number(number) => {
            let raw = number.as_i64()?;
            if raw == 0 {
                return None;
            }
            let millis = if raw.abs() >= 1_000_000_000_000 {
                raw
            } else {
                raw * 1000
            };
            Some(Timestamp::from_millis(millis))
        }
        Value::String(text) => parse_rfc3339(text),
        _ => None,
    }
}

/// Minimal RFC3339 parser returning epoch milliseconds.
fn parse_rfc3339(text: &str) -> Option<Timestamp> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let split = text.find(['T', 't', ' '])?;
    let date = &text[..split];
    let rest = &text[split + 1..];

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;

    let (time_str, tz_offset_secs) = split_timezone(rest)?;
    let mut time_parts = time_str.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next().unwrap_or("0").parse().ok()?;
    let (second, frac_millis) = parse_seconds(time_parts.next().unwrap_or("0"))?;

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3_600 + minute * 60 + second - tz_offset_secs;
    Some(Timestamp::from_millis(secs * 1000 + frac_millis))
}

fn split_timezone(rest: &str) -> Option<(&str, i64)> {
    if let Some(stripped) = rest.strip_suffix(['Z', 'z']) {
        return Some((stripped, 0));
    }
    for (index, ch) in rest.char_indices() {
        if (ch == '+' || ch == '-') && index > 0 {
            let offset = parse_offset(&rest[index..])?;
            return Some((&rest[..index], offset));
        }
    }
    Some((rest, 0))
}

fn parse_offset(offset: &str) -> Option<i64> {
    let sign = match offset.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits = &offset[1..];
    let cleaned: String = digits.chars().filter(|ch| *ch != ':').collect();
    let (hours, minutes) = match cleaned.len() {
        2 => (cleaned.parse::<i64>().ok()?, 0),
        4 => (cleaned[..2].parse().ok()?, cleaned[2..].parse().ok()?),
        _ => return None,
    };
    Some(sign * (hours * 3_600 + minutes * 60))
}

fn parse_seconds(field: &str) -> Option<(i64, i64)> {
    if let Some(dot) = field.find('.') {
        let whole: i64 = field[..dot].parse().ok()?;
        let mut frac = field[dot + 1..].to_string();
        frac.truncate(3);
        while frac.len() < 3 {
            frac.push('0');
        }
        let millis: i64 = frac.parse().ok()?;
        Some((whole, millis))
    } else {
        Some((field.parse().ok()?, 0))
    }
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_and_unix_seconds() {
        let utc = parse_timestamp(&Value::String("1970-01-01T00:00:01Z".to_string()));
        assert_eq!(utc, Some(Timestamp::from_millis(1000)));
        let offset = parse_timestamp(&Value::String("1970-01-01T01:00:00+01:00".to_string()));
        assert_eq!(offset, Some(Timestamp::from_millis(0)));
        let secs = parse_timestamp(&Value::Number(serde_json::Number::from(1)));
        assert_eq!(secs, Some(Timestamp::from_millis(1000)));
    }
}
