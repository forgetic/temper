//! Small helpers for bounded structured stderr events.

use std::collections::BTreeMap;

use serde_json::Value;

/// Default bound for scalar event fields that originate outside Smith.
pub(crate) const FIELD_PREVIEW_CHARS: usize = 200;
/// Default bound for free-form model/operator text in events.
pub(crate) const REASON_PREVIEW_CHARS: usize = 200;

pub(crate) const REDACTED: &str = "<redacted>";

/// A stable JSON event renderer for Smith stderr logs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StructuredEvent {
    fields: BTreeMap<String, Value>,
}

impl StructuredEvent {
    /// Starts a new event with the stable `event` field.
    pub(crate) fn new(event: impl AsRef<str>) -> Self {
        let mut fields = BTreeMap::new();
        fields.insert(
            "event".to_string(),
            Value::String(preview(event.as_ref(), FIELD_PREVIEW_CHARS)),
        );
        Self { fields }
    }

    /// Adds a bounded string field.
    pub(crate) fn str(mut self, key: &str, value: impl AsRef<str>) -> Self {
        self.fields.insert(
            key.to_string(),
            Value::String(preview(value.as_ref(), FIELD_PREVIEW_CHARS)),
        );
        self
    }

    /// Adds a bounded string field when the value is present.
    pub(crate) fn opt_str(mut self, key: &str, value: Option<&str>) -> Self {
        if let Some(value) = value {
            self.fields.insert(
                key.to_string(),
                Value::String(preview(value, FIELD_PREVIEW_CHARS)),
            );
        }
        self
    }

    /// Adds a boolean field.
    pub(crate) fn bool(mut self, key: &str, value: bool) -> Self {
        self.fields.insert(key.to_string(), Value::Bool(value));
        self
    }

    /// Adds a `usize` field.
    pub(crate) fn usize(mut self, key: &str, value: usize) -> Self {
        self.fields
            .insert(key.to_string(), serde_json::json!(value));
        self
    }

    /// Adds a `u64` field.
    pub(crate) fn u64(mut self, key: &str, value: u64) -> Self {
        self.fields
            .insert(key.to_string(), serde_json::json!(value));
        self
    }

    /// Adds a bounded string-array field.
    pub(crate) fn strings<I, S>(mut self, key: &str, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let values = values
            .into_iter()
            .map(|value| Value::String(preview(value.as_ref(), FIELD_PREVIEW_CHARS)))
            .collect();
        self.fields.insert(key.to_string(), Value::Array(values));
        self
    }

    /// Renders the event as one compact JSON object.
    pub(crate) fn render(&self) -> String {
        serde_json::to_string(&self.fields).expect("structured event fields serialize")
    }
}

/// Returns a single-line preview bounded by `max_chars` Unicode scalar values.
pub(crate) fn preview(input: &str, max_chars: usize) -> String {
    let collapsed = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }

    let mut output = collapsed.chars().take(max_chars).collect::<String>();
    output.push('…');
    output
}

/// Redacts obvious secret-like keys/values, then returns a bounded single-line
/// preview for free-form model/operator text.
pub(crate) fn redacted_preview(input: &str, max_chars: usize) -> String {
    preview(&redact_secret_like_text(input), max_chars)
}

/// Returns true when the central redactor would mask some part of the text.
pub(crate) fn contains_secret_like_text(input: &str) -> bool {
    let collapsed = input.split_whitespace().collect::<Vec<_>>().join(" ");
    redact_secret_like_text(input) != collapsed
}

fn redact_secret_like_text(input: &str) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;

    for raw in input.split_whitespace() {
        if redact_next {
            if is_bearer_marker(raw) {
                output.push(raw.to_string());
                continue;
            }
            output.push(redacted_token(raw));
            redact_next = false;
            continue;
        }

        if is_bearer_marker(raw) {
            output.push(raw.to_string());
            redact_next = true;
            continue;
        }

        match secret_assignment(raw) {
            SecretAssignment::Inline(redacted) => {
                output.push(redacted);
                continue;
            }
            SecretAssignment::KeyOnly => {
                output.push(raw.to_string());
                redact_next = true;
                continue;
            }
            SecretAssignment::None => {}
        }

        if looks_like_secret_value(raw) {
            output.push(redacted_token(raw));
        } else {
            output.push(raw.to_string());
        }
    }

    output.join(" ")
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SecretAssignment {
    Inline(String),
    KeyOnly,
    None,
}

fn secret_assignment(raw: &str) -> SecretAssignment {
    for (index, delimiter) in raw.char_indices().filter(|(_, ch)| matches!(ch, '=' | ':')) {
        let key = raw[..index].trim_matches(secret_key_wrapper);
        if !is_secret_like_key(key) {
            continue;
        }
        let value = &raw[index + delimiter.len_utf8()..];
        if value.trim_matches(secret_value_wrapper).is_empty() {
            return SecretAssignment::KeyOnly;
        }
        return SecretAssignment::Inline(format!("{}{}", &raw[..=index], REDACTED));
    }
    SecretAssignment::None
}

fn is_bearer_marker(raw: &str) -> bool {
    raw.trim_matches(secret_key_wrapper)
        .eq_ignore_ascii_case("bearer")
}

fn is_secret_like_key(key: &str) -> bool {
    let compact = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();

    if compact.is_empty() {
        return false;
    }

    compact.contains("token")
        || compact.contains("secret")
        || compact.contains("password")
        || compact.contains("passwd")
        || compact.contains("apikey")
        || compact.contains("credential")
        || compact.contains("privatekey")
        || compact == "authorization"
        || compact == "bearer"
        || compact == "access"
        || compact == "refresh"
        || compact == "authfile"
        || compact == "authpath"
        || compact == "keypath"
}

fn looks_like_secret_value(raw: &str) -> bool {
    let value = raw.trim_matches(secret_value_wrapper);
    let lower = value.to_ascii_lowercase();
    value.len() >= 12
        && (lower.starts_with("sk-")
            || lower.starts_with("sk_")
            || lower.starts_with("ghp_")
            || lower.starts_with("github_pat_")
            || lower.starts_with("glpat-")
            || lower.starts_with("xoxb-")
            || lower.starts_with("xoxp-")
            || value.starts_with("AIza")
            || lower.starts_with("ya29.")
            || lower.starts_with("eyj"))
}

fn redacted_token(raw: &str) -> String {
    let prefix = raw
        .chars()
        .take_while(|ch| ch.is_ascii_punctuation() && !matches!(ch, '<' | '[' | '('))
        .collect::<String>();
    let suffix = raw
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_punctuation() && !matches!(ch, '>' | ']' | ')'))
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}{REDACTED}{suffix}")
}

fn secret_key_wrapper(ch: char) -> bool {
    ch.is_ascii_whitespace()
        || matches!(
            ch,
            '"' | '\'' | '`' | '{' | '}' | '[' | ']' | '(' | ')' | ',' | ';'
        )
}

fn secret_value_wrapper(ch: char) -> bool {
    ch.is_ascii_whitespace()
        || matches!(
            ch,
            '"' | '\'' | '`' | '{' | '}' | '[' | ']' | '(' | ')' | ',' | ';'
        )
}

/// Extracts a scalar JSON value as a bounded event string.
pub(crate) fn scalar_preview(value: Option<&Value>) -> Option<String> {
    let raw = match value? {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null | Value::Array(_) | Value::Object(_) => return None,
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(preview(trimmed, FIELD_PREVIEW_CHARS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_stable_json_with_sorted_keys() {
        let rendered = StructuredEvent::new("smith.test")
            .str("zeta", "last")
            .usize("count", 2)
            .strings("ids", ["one", "two"])
            .render();

        assert_eq!(
            rendered,
            r#"{"count":2,"event":"smith.test","ids":["one","two"],"zeta":"last"}"#
        );
    }

    #[test]
    fn preview_collapses_whitespace_and_truncates_on_char_boundary() {
        let text = "first\nsecond\t🙂 third";
        assert_eq!(preview(text, 14), "first second 🙂…");
    }

    #[test]
    fn scalar_preview_tolerates_missing_or_non_scalar_json() {
        let value = serde_json::json!({"nested": true});
        assert_eq!(
            scalar_preview(Some(&serde_json::json!(42))).as_deref(),
            Some("42")
        );
        assert_eq!(
            scalar_preview(Some(&serde_json::json!("  id-1  "))).as_deref(),
            Some("id-1")
        );
        assert_eq!(scalar_preview(Some(&value)), None);
        assert_eq!(scalar_preview(None), None);
    }

    #[test]
    fn redacted_preview_masks_secret_like_keys_and_values() {
        let text = r#"{"access_token": "tok-123", "auth_file": "/home/free/.pi/agent/auth.json", "body": "Bearer sk-secret-do-not-log"} refresh_token=rotating ghp_secretvalue"#;

        let preview = redacted_preview(text, 500);

        assert!(preview.contains("access_token"));
        assert!(preview.contains(REDACTED));
        assert!(!preview.contains("tok-123"));
        assert!(!preview.contains("auth.json"));
        assert!(!preview.contains("sk-secret-do-not-log"));
        assert!(!preview.contains("refresh_token=rotating"));
        assert!(!preview.contains("ghp_secretvalue"));

        let compact = redacted_preview(
            r#"{"auth_file":"/home/free/.pi/agent/auth.json","body":"password=hunter2"}"#,
            500,
        );
        assert!(!compact.contains("auth.json"));
        assert!(!compact.contains("hunter2"));
    }

    #[test]
    fn redacted_preview_bounds_after_redaction() {
        let text = format!("password=hunter2 {}TAIL", "x".repeat(50));
        let preview = redacted_preview(&text, 12);

        assert_eq!(preview.chars().count(), 13);
        assert!(preview.ends_with('…'));
        assert!(!preview.contains("hunter2"));
        assert!(!preview.contains("TAIL"));
    }

    #[test]
    fn contains_secret_like_text_reports_redactor_matches() {
        assert!(contains_secret_like_text(
            "Authorization: Bearer sk-secret-value"
        ));
        assert!(contains_secret_like_text("github_pat_secretvalue"));
        assert!(!contains_secret_like_text(
            "decision/work-item/repo:20/acme"
        ));
    }
}
