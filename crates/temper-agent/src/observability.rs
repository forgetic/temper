//! Bounded, redacted field-rendering helpers for agent observability lines.
//!
//! `preview` / `redacted_preview` are temper-agent's local copy of the
//! redaction rule (the depgraph forbids depending on `temper-log`, where the
//! same idea lives): they bound free-form model/operator text to a single,
//! length-limited line and mask secret-like keys/values.
//!
//! NOTE (agent-log-cleanup, §8): agent observability emits **real `tracing`
//! fields** on `target: "temper::agent"` rather than a JSON string in the
//! message body. The old `StructuredEvent` JSON-string builder has been
//! deleted; only the bounded/redaction helpers below survive.

pub(crate) const REDACTED: &str = "<redacted>";

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_collapses_whitespace_and_truncates_on_char_boundary() {
        let text = "first\nsecond\t🙂 third";
        assert_eq!(preview(text, 14), "first second 🙂…");
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
}
