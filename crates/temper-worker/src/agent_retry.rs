//! Bounded retry policy for provider/model failures at the worker → agent boundary.
//!
//! The agent loop already retries individual streaming calls when it can see the
//! typed provider error. This module is the outer guard for failures that cross
//! the process/runner boundary as flattened text (for example a `server_error`
//! from an OpenAI/Codex-style provider that made the agent process exit before a
//! `WorkspaceResult` was written). It deliberately recognizes only provider or
//! model transport/server symptoms; deterministic configuration, request, model,
//! credential, result-file, and contract failures must pass through without an
//! immediate worker-side retry.

use std::time::Duration;

/// Prefix the in-tree agent may include on stderr when it has already classified
/// a failure as a retryable provider/model fault.
pub const RETRYABLE_PROVIDER_ERROR_MARKER: &str = "retryable-provider:";

/// Short, bounded retry schedule for retryable provider/model boundary failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentRetryPolicy {
    /// Total attempts, including the first try. Values below one are clamped to
    /// one so a malformed policy can never spin.
    pub max_attempts: usize,
    /// Backoff before the first retry. Later retries double this value until
    /// [`max_delay`](Self::max_delay).
    pub base_delay: Duration,
    /// Upper bound for the exponential component of one delay.
    pub max_delay: Duration,
    /// Maximum deterministic jitter added to each delay. The jitter is derived
    /// from the correlation key and attempt number so it is stable and needs no
    /// process-global RNG state.
    pub jitter: Duration,
}

impl Default for AgentRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(2),
            jitter: Duration::from_millis(100),
        }
    }
}

impl AgentRetryPolicy {
    /// Effective total attempt cap, including the first try.
    pub fn max_attempts(self) -> usize {
        self.max_attempts.max(1)
    }

    /// Whether the completed attempt should be retried immediately.
    ///
    /// `completed_attempt` is one-based: after the first failed try, pass `1`.
    pub fn should_retry_provider_failure(self, completed_attempt: usize, message: &str) -> bool {
        completed_attempt < self.max_attempts() && is_retryable_provider_error(message)
    }

    /// Delay before the next attempt after `completed_attempt` failed.
    ///
    /// The exponential component uses attempts 1, 2, 3... as multipliers
    /// 1×, 2×, 4×... and is capped at [`max_delay`](Self::max_delay). Jitter is
    /// bounded by [`jitter`](Self::jitter) and deterministic per correlation key.
    pub fn delay_before_retry(self, completed_attempt: usize, correlation_key: &str) -> Duration {
        let exponent = completed_attempt.saturating_sub(1).min(10);
        let multiplier = 1u32 << exponent;
        let exponential = (self.base_delay * multiplier).min(self.max_delay);
        exponential + deterministic_jitter(self.jitter, correlation_key, completed_attempt)
    }
}

/// Returns true only for provider/model failures that a fresh attempt can
/// plausibly survive: transport drops, timeouts/stalls, rate limits, and server
/// errors (including OpenAI/Codex `server_error`).
pub fn is_retryable_provider_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    if lower.trim().is_empty() || contains_non_retryable_provider_error(&lower) {
        return false;
    }
    if lower.contains(RETRYABLE_PROVIDER_ERROR_MARKER) {
        return true;
    }
    if status_numbers(&lower).any(is_retryable_status) {
        return true;
    }
    RETRYABLE_FRAGMENTS
        .iter()
        .any(|fragment| lower.contains(fragment))
}

const RETRYABLE_FRAGMENTS: &[&str] = &[
    "server_error",
    "internal_server_error",
    "service_unavailable",
    "gateway timeout",
    "bad gateway",
    "overloaded",
    "rate_limit",
    "rate limit",
    "too many requests",
    "connection reset",
    "connection refused",
    "connection closed",
    "connection aborted",
    "broken pipe",
    "timed out",
    "timeout",
    "temporarily unavailable",
    "temporary failure",
    "dns",
    "tls handshake",
    "transport",
    "no response start",
    "stream ended without a terminal",
    "stream stalled",
    "request stalled",
    "eof",
];

fn contains_non_retryable_provider_error(lower: &str) -> bool {
    if status_numbers(lower).any(is_non_retryable_status) {
        return true;
    }
    if lower.contains("invalid_request")
        || lower.contains("invalid request")
        || lower.contains("bad_request")
        || lower.contains("bad request")
        || lower.contains("malformed request")
        || lower.contains("unsupported model")
        || lower.contains("model_not_found")
        || lower.contains("model not found")
        || lower.contains("does not exist")
        || lower.contains("do not have access")
    {
        return true;
    }
    if (lower.contains("model") || lower.contains("alias"))
        && (lower.contains("is not available")
            || lower.contains("not available")
            || lower.contains("unavailable"))
    {
        return true;
    }
    lower.contains("unauthorized")
        || lower.contains("unauthenticated")
        || lower.contains("forbidden")
        || lower.contains("permission denied")
        || lower.contains("bad credential")
        || lower.contains("invalid credential")
        || lower.contains("invalid api key")
        || lower.contains("incorrect api key")
        || lower.contains("api key expired")
        || lower.contains("authentication failed")
}

fn is_retryable_status(status: u16) -> bool {
    status == 408 || status == 409 || status == 425 || status == 429 || (500..=599).contains(&status)
}

fn is_non_retryable_status(status: u16) -> bool {
    matches!(status, 400 | 401 | 403 | 404 | 422)
}

fn status_numbers(message: &str) -> impl Iterator<Item = u16> + '_ {
    message
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|token| token.len() == 3)
        .filter_map(|token| token.parse::<u16>().ok())
}

fn deterministic_jitter(
    max_jitter: Duration,
    correlation_key: &str,
    completed_attempt: usize,
) -> Duration {
    if max_jitter.is_zero() {
        return Duration::ZERO;
    }
    let max_ms = u64::try_from(max_jitter.as_millis()).unwrap_or(u64::MAX);
    if max_ms == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(stable_hash(correlation_key, completed_attempt) % (max_ms + 1))
}

fn stable_hash(correlation_key: &str, completed_attempt: usize) -> u64 {
    // FNV-1a with the attempt mixed in. Tiny, deterministic, and adequate for
    // spreading same-second retries without adding an RNG dependency.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in correlation_key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for byte in completed_attempt.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_server_errors_are_retryable() {
        assert!(is_retryable_provider_error(
            "OpenAI API error: server_error: upstream overloaded"
        ));
        assert!(is_retryable_provider_error(
            "agent stopped abnormally: HTTP 503 service_unavailable"
        ));
        assert!(is_retryable_provider_error("status=429 rate_limit"));
        assert!(is_retryable_provider_error("connection reset by peer"));
    }

    #[test]
    fn deterministic_provider_failures_are_not_retryable() {
        for message in [
            "HTTP 400 invalid_request: max_tokens too large",
            "401 unauthorized: invalid API key",
            "model `gpt-missing` is unavailable: model_not_found",
            "unsupported model family",
            "agent result file is not valid JSON",
        ] {
            assert!(
                !is_retryable_provider_error(message),
                "{message:?} should not retry"
            );
        }
    }

    #[test]
    fn retry_policy_bounds_attempts_and_delay() {
        let policy = AgentRetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(15),
            jitter: Duration::from_millis(5),
        };
        assert!(policy.should_retry_provider_failure(1, "server_error"));
        assert!(policy.should_retry_provider_failure(2, "server_error"));
        assert!(!policy.should_retry_provider_failure(3, "server_error"));
        assert!(!policy.should_retry_provider_failure(1, "invalid_request"));

        let first = policy.delay_before_retry(1, "pr-for-code-7");
        let later = policy.delay_before_retry(3, "pr-for-code-7");
        assert!(first >= Duration::from_millis(10));
        assert!(first <= Duration::from_millis(15));
        assert!(later >= Duration::from_millis(15));
        assert!(later <= Duration::from_millis(20));
    }
}
