// SPDX-License-Identifier: MPL-2.0
//! Tolerant timestamp parsing for Forgejo Actions payloads.
//!
//! Forgejo reports Actions timestamps inconsistently: the API `tasks` endpoint
//! emits RFC3339 strings (`created_at`), while run payloads can carry unix-epoch
//! integers (`created`, `updated`). This serde helper accepts either form and
//! normalizes to a portable [`DateTime<Utc>`], so the CI DTOs in
//! [`crate::types`] tolerate both shapes without a separate parser.

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// serde `deserialize_with` helper for an optional Actions timestamp.
///
/// Returns `None` for a missing field, JSON `null`, an empty string, or a zero
/// integer; otherwise parses an RFC3339 string or a unix-epoch integer.
pub(crate) fn deserialize_flexible_opt_datetime<'de, D>(
    deserializer: D,
) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(parse_flexible))
}

/// Parses an RFC3339 string or a unix-epoch (seconds or milliseconds) integer.
fn parse_flexible(value: &Value) -> Option<DateTime<Utc>> {
    match value {
        Value::Number(number) => {
            let raw = number.as_i64()?;
            if raw == 0 {
                return None;
            }
            // Heuristic: very large magnitudes are already milliseconds.
            let millis = if raw.abs() >= 1_000_000_000_000 {
                raw
            } else {
                raw.checked_mul(1000)?
            };
            Utc.timestamp_millis_opt(millis).single()
        }
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            DateTime::parse_from_rfc3339(text)
                .ok()
                .map(|parsed| parsed.with_timezone(&Utc))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_string() {
        let parsed = parse_flexible(&Value::String("1970-01-01T00:00:01Z".to_string()));
        assert_eq!(parsed, Utc.timestamp_millis_opt(1000).single());
    }

    #[test]
    fn parses_rfc3339_with_offset() {
        let parsed = parse_flexible(&Value::String("1970-01-01T01:00:00+01:00".to_string()));
        assert_eq!(parsed, Utc.timestamp_millis_opt(0).single());
    }

    #[test]
    fn parses_unix_seconds_and_millis() {
        let secs = parse_flexible(&Value::Number(serde_json::Number::from(1)));
        assert_eq!(secs, Utc.timestamp_millis_opt(1000).single());
        let millis = parse_flexible(&Value::Number(serde_json::Number::from(
            1_700_000_000_000i64,
        )));
        assert_eq!(millis, Utc.timestamp_millis_opt(1_700_000_000_000).single());
    }

    #[test]
    fn treats_zero_and_blank_as_absent() {
        assert_eq!(
            parse_flexible(&Value::Number(serde_json::Number::from(0))),
            None
        );
        assert_eq!(parse_flexible(&Value::String("  ".to_string())), None);
        assert_eq!(parse_flexible(&Value::Null), None);
    }
}
