// SPDX-License-Identifier: MPL-2.0

//! Bounded, secret-redacting text previews for free-text log fields.
//!
//! Issue titles and agent summaries are operator-authored free text; they must
//! be bounded (one line, length-capped) and scrubbed of anything that looks like
//! a credential before they land in a log field or human message (§8 — the
//! redaction rule survives, re-homed from the runner into the event model).
//!
//! This is the re-home of `temper-runner`'s `observability/redact.rs`. The
//! functions and the [`REDACTED`] marker keep their behavior and tests verbatim;
//! WI-2 migrates the runner call sites to this copy and removes the duplicate.

/// Marker substituted when text looks like it may contain a credential/token.
pub const REDACTED: &str = "<redacted>";

/// Returns a one-line, character-bounded preview of arbitrary text.
///
/// Internal whitespace is collapsed to single spaces (so multi-line input
/// renders on one line); the result is truncated to at most `max_chars`
/// characters, appending an ellipsis (`…`) when it had to cut. `max_chars == 0`
/// yields the empty string.
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

/// Returns a [`bounded_preview`], replacing secret-like text with [`REDACTED`].
///
/// When the input contains a credential-shaped substring (a `token=`, a
/// `Bearer ` header, a PEM header, …) the whole preview collapses to the
/// [`REDACTED`] marker rather than risk leaking a fragment of the secret.
pub fn redacted_preview(text: &str, max_chars: usize) -> String {
    if contains_secret_like(text) {
        REDACTED.to_string()
    } else {
        bounded_preview(text, max_chars)
    }
}

/// Converts possibly non-UTF-8 bytes to a redacted, bounded preview.
///
/// Invalid UTF-8 is replaced lossily before redaction so raw forge/agent byte
/// payloads can still be previewed safely.
pub fn redacted_lossy_preview(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    redacted_preview(&text, max_chars)
}

/// Heuristic: does the text contain a credential-shaped substring?
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
