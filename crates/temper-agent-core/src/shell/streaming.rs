//! Model streaming for the agent shell: turning a tongs provider stream into a
//! single [`AgentCompletion`], with liveness timeouts and transient-fault retry.
//!
//! A model turn streams deltas (forwarded to the [`EventSink`] for live
//! observers) and ends with a terminal `Done`/`Error` event carrying the
//! assembled assistant message. Transport faults (dropped connection, 429/5xx,
//! a stalled socket) are retried with capped exponential backoff before the
//! turn is given up on; a model-chosen error stop is surfaced as-is, not
//! retried.

use futures::StreamExt;
use tongs::model::{AssistantMessage, Message, StopReason, StreamEvent};
use tongs::provider::{Context, Provider, StreamOptions, ToolDef};

use crate::machine::{AgentCompletion, AgentEvent, StreamDelta};
use crate::shell::EventSink;

/// How long to wait for the provider to start responding (connect + TLS +
/// request write + first stream event) before treating the call as stalled.
const STREAM_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How long to wait between stream events once a response has started. A live
/// model turn emits deltas continuously; this only fires when the stream goes
/// silent (a dead socket), so it never cuts off a slow-but-progressing turn.
const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Maximum number of *additional* attempts after the first for a transient
/// model-call failure (so up to `MAX_STREAM_RETRIES + 1` total tries). Sized to
/// ride out a short burst of connection resets (observed on large request
/// bodies under load) without giving up on the turn.
const MAX_STREAM_RETRIES: usize = 6;

/// Base backoff before the first retry; doubles each subsequent attempt (capped).
const STREAM_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);

/// Upper bound on a single backoff so exponential growth does not stall a turn
/// for minutes on the later attempts.
const STREAM_RETRY_BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(8);

/// Retry budget/timing for transient provider failures in one model turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamRetryConfig {
    /// Additional attempts after the first failed attempt.
    pub max_retries: usize,
    /// Backoff before the first retry; doubled on later retries.
    pub base_backoff: std::time::Duration,
    /// Per-retry cap applied after exponential growth.
    pub max_backoff: std::time::Duration,
}

impl Default for StreamRetryConfig {
    fn default() -> Self {
        Self {
            max_retries: MAX_STREAM_RETRIES,
            base_backoff: STREAM_RETRY_BACKOFF,
            max_backoff: STREAM_RETRY_BACKOFF_MAX,
        }
    }
}

impl StreamRetryConfig {
    fn backoff(self, attempt: usize) -> std::time::Duration {
        (self.base_backoff * (1u32 << attempt.min(5))).min(self.max_backoff)
    }
}

#[cfg(feature = "test-support")]
static STREAM_RETRY_CONFIG_OVERRIDE: std::sync::Mutex<Option<StreamRetryConfig>> =
    std::sync::Mutex::new(None);

/// Process-local guard for a temporary stream retry override.
///
/// This is test-support API for hermetic integration tests that need to exhaust
/// the retry budget without waiting for production-scale backoff delays.
#[cfg(feature = "test-support")]
#[must_use = "the stream retry override is reset when the guard is dropped"]
pub struct StreamRetryConfigOverrideGuard {
    previous: Option<StreamRetryConfig>,
}

#[cfg(feature = "test-support")]
impl Drop for StreamRetryConfigOverrideGuard {
    fn drop(&mut self) {
        let mut guard = STREAM_RETRY_CONFIG_OVERRIDE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = self.previous;
    }
}

/// Overrides the process-local stream retry policy until the returned guard is
/// dropped.
#[cfg(feature = "test-support")]
pub fn install_stream_retry_config_override(
    config: StreamRetryConfig,
) -> StreamRetryConfigOverrideGuard {
    let mut guard = STREAM_RETRY_CONFIG_OVERRIDE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = guard.replace(config);
    StreamRetryConfigOverrideGuard { previous }
}

fn stream_retry_config() -> StreamRetryConfig {
    #[cfg(feature = "test-support")]
    {
        if let Some(config) = *STREAM_RETRY_CONFIG_OVERRIDE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            return config;
        }
    }
    StreamRetryConfig::default()
}

/// Streams one model response and collapses it into a completion. The terminal
/// `Done` / `Error` stream event carries the final assistant message; a
/// transport-layer failure (the provider call itself erroring, or the stream
/// ending without a terminal event) becomes [`AgentCompletion::LlmFailed`].
pub(super) async fn stream_to_completion(
    provider: &dyn Provider,
    system_prompt: Option<&str>,
    messages: &[Message],
    tool_defs: &[ToolDef],
    stream_options: &StreamOptions,
    events: &dyn EventSink,
) -> AgentCompletion {
    let context = Context {
        system_prompt: system_prompt.map(std::borrow::Cow::Borrowed),
        messages: std::borrow::Cow::Borrowed(messages),
        tools: std::borrow::Cow::Borrowed(tool_defs),
    };

    // Transient failures (a dropped connection, a 429/5xx, an overloaded
    // provider) are retried with backoff rather than killing the turn — and, in
    // a parallel sub-agent fan-out, a whole run. One flaky socket should not be
    // fatal; this mirrors the resilience the Claude Code CLI gets for free.
    // Retry only happens before any terminal message is assembled, so a model
    // turn never double-emits. A terminal provider error message (the model
    // chose to stop with an error) is NOT a transport fault and is surfaced as-is.
    let mut attempt = 0usize;
    let retry_config = stream_retry_config();
    loop {
        match stream_one_attempt(provider, &context, stream_options, events).await {
            StreamAttempt::Responded(message) => {
                return AgentCompletion::LlmResponded(message);
            }
            StreamAttempt::Failed { reason, retryable } => {
                let will_retry = retryable && attempt < retry_config.max_retries;
                events.emit(AgentEvent::ModelCallFailed {
                    reason: reason.clone(),
                    will_retry,
                });
                if will_retry {
                    temper_agent_io::sleep_for(retry_config.backoff(attempt)).await;
                    attempt += 1;
                    continue;
                }
                return AgentCompletion::LlmFailed(reason);
            }
        }
    }
}

/// Outcome of one streaming attempt.
enum StreamAttempt {
    /// The turn produced a terminal assistant message (normal or model-chosen error).
    Responded(AssistantMessage),
    /// The attempt failed before a terminal message; `retryable` says whether a
    /// fresh attempt could plausibly succeed (transport / overload faults).
    Failed { reason: String, retryable: bool },
}

/// Runs one streaming attempt with liveness timeouts. Live deltas are forwarded
/// to `events` so observers see tokens/tool-calls in real time; the terminal
/// `Done`/`Error` event carries the assembled message the machine acts on.
async fn stream_one_attempt(
    provider: &dyn Provider,
    context: &Context<'_>,
    stream_options: &StreamOptions,
    events: &dyn EventSink,
) -> StreamAttempt {
    // Liveness guard: a healthy model turn emits stream events steadily, but the
    // provider HTTP path (skein) has no socket read timeout, so a stalled
    // connection would block this task — and, in a parallel `investigate`
    // fan-out, the whole batch (the machine waits for every sub-agent before it
    // can advance) — forever. We bound each await so a stall fails the attempt
    // (retryably) instead of deadlocking. The window only trips on *no event at
    // all* for the interval, never on a slow-but-alive stream.
    let mut stream = match temper_agent_io::timeout(
        STREAM_CONNECT_TIMEOUT,
        provider.stream(context, stream_options),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            let retryable = is_retryable(&error);
            return StreamAttempt::Failed {
                reason: error.to_string(),
                retryable,
            };
        }
        Err(_) => {
            return StreamAttempt::Failed {
                reason: format!(
                    "model request stalled: no response start within {}s",
                    STREAM_CONNECT_TIMEOUT.as_secs()
                ),
                retryable: true,
            };
        }
    };

    loop {
        let event = match temper_agent_io::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await {
            Ok(Some(event)) => event,
            Ok(None) => {
                return StreamAttempt::Failed {
                    reason: "model stream ended without a terminal Done/Error event".to_string(),
                    // A clean EOF with no terminal event is usually a dropped
                    // connection mid-stream — worth one more try.
                    retryable: true,
                };
            }
            Err(_) => {
                return StreamAttempt::Failed {
                    reason: format!(
                        "model stream stalled: no event for {}s",
                        STREAM_IDLE_TIMEOUT.as_secs()
                    ),
                    retryable: true,
                };
            }
        };
        match event {
            Ok(StreamEvent::Done { message, .. }) => {
                return StreamAttempt::Responded(message);
            }
            Ok(StreamEvent::Error { error, .. }) => {
                // The provider produced a terminal error message; surface it as
                // an assistant message with an error stop reason so the machine
                // records it and stops cleanly. This is the model's decision, not
                // a transport fault, so it is not retried.
                let mut message = error;
                message.stop_reason = StopReason::Error;
                return StreamAttempt::Responded(message);
            }
            Ok(StreamEvent::TextDelta { delta, .. }) => {
                events.emit(AgentEvent::StreamDelta(StreamDelta::Text(delta)));
            }
            Ok(StreamEvent::ThinkingDelta { delta, .. }) => {
                events.emit(AgentEvent::StreamDelta(StreamDelta::Thinking(delta)));
            }
            Ok(StreamEvent::ToolCallEnd { tool_call, .. }) => {
                events.emit(AgentEvent::StreamDelta(StreamDelta::ToolCall {
                    id: tool_call.id,
                    name: tool_call.name,
                }));
            }
            Ok(_) => {}
            Err(error) => {
                let retryable = is_retryable(&error);
                return StreamAttempt::Failed {
                    reason: error.to_string(),
                    retryable,
                };
            }
        }
    }
}

/// Whether a provider error is a transient transport/overload fault worth
/// retrying, versus a deterministic failure (bad request, auth, decode) that a
/// retry cannot fix.
fn is_retryable(error: &tongs::error::Error) -> bool {
    use tongs::error::Error;
    match error {
        // Transport faults: DNS/TCP/TLS/malformed HTTP — typically transient.
        Error::Http(_) => true,
        // Retry rate-limit and server-side faults; never retry 4xx the client
        // must fix (400 invalid_request, 401/403 auth, 404 model-unavailable).
        Error::Api { status, .. } => *status == 429 || (500..=599).contains(status),
        // Deterministic: a retry would fail identically.
        Error::Auth(_) | Error::Decode(_) | Error::Tool(_) | Error::Aborted | Error::Other(_) => {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_retryable;
    use tongs::error::Error;

    #[test]
    fn transport_and_overload_errors_are_retryable() {
        assert!(is_retryable(&Error::Http(
            "connection reset by peer".into()
        )));
        assert!(is_retryable(&Error::Api {
            status: 429,
            message: "rate_limit".into()
        }));
        assert!(is_retryable(&Error::Api {
            status: 503,
            message: "overloaded".into()
        }));
        assert!(is_retryable(&Error::Api {
            status: 529,
            message: "overloaded".into()
        }));
    }

    #[test]
    fn client_and_deterministic_errors_are_not_retryable() {
        // 400 invalid_request (e.g. max_tokens over a model's cap) — a retry
        // fails identically; the request itself must change.
        assert!(!is_retryable(&Error::Api {
            status: 400,
            message: "max_tokens too large".into()
        }));
        assert!(!is_retryable(&Error::Api {
            status: 401,
            message: "unauthorized".into()
        }));
        assert!(!is_retryable(&Error::Api {
            status: 404,
            message: "model not available".into()
        }));
        assert!(!is_retryable(&Error::Auth("expired".into())));
        assert!(!is_retryable(&Error::Decode("bad json".into())));
        assert!(!is_retryable(&Error::Aborted));
    }
}
