//! Bounded, secret-redacting text previews for observability events.

/// Marker used when text looks like it may contain a credential or token.
pub const REDACTED: &str = "<redacted>";

/// Returns a one-line, character-bounded preview of arbitrary text.
pub fn bounded_preview(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let mut preview = normalized.chars().take(max_chars - 1).collect::<String>();
    preview.push('…');
    preview
}

/// Returns a bounded preview, replacing secret-like text with [`REDACTED`].
pub fn redacted_preview(text: &str, max_chars: usize) -> String {
    if contains_secret_like(text) {
        REDACTED.to_string()
    } else {
        bounded_preview(text, max_chars)
    }
}

/// Converts possibly non-UTF-8 bytes to a redacted, bounded preview.
pub fn redacted_lossy_preview(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    redacted_preview(&text, max_chars)
}

fn contains_secret_like(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    [
        "token=",
        "token:",
        "password=",
        "password:",
        "secret=",
        "secret:",
        "authorization=",
        "authorization:",
        "bearer ",
        "api_key=",
        "api-key=",
        "auth=",
        "-----begin ",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_truncates_on_character_boundaries() {
        assert_eq!(
            bounded_preview("hello\nthere   friend", 14),
            "hello there f…"
        );
        assert_eq!(bounded_preview("éclair", 5), "écla…");
    }

    #[test]
    fn secret_like_previews_are_excluded() {
        assert_eq!(
            redacted_preview("TEMPER_FORGEJO_TOKEN=super-secret", 80),
            REDACTED
        );
        assert_eq!(
            redacted_lossy_preview(b"Authorization: bearer abc123", 80),
            REDACTED
        );
        assert_eq!(redacted_preview("ordinary reason", 80), "ordinary reason");
    }
}
