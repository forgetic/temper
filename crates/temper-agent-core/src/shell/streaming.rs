//! Model streaming for the agent shell: turning a tongs provider stream into a
//! single model task outcome, with liveness timeouts and transient-fault retry.
//!
//! A model turn streams deltas (forwarded to the [`EventSink`] for live
//! observers) and ends with a terminal `Done`/`Error` event. `Done` yields the
//! assembled assistant response; `Error` yields a typed provider diagnostic and
//! is always a failed attempt. Retryable transport/provider faults (dropped
//! connection, 429/5xx, a stalled socket) use the existing capped exponential
//! backoff before the turn is given up on.

use std::future::Future;
use std::time::Duration;

use futures::StreamExt;
use futures::future::Either;
use tongs::model::{AssistantMessage, Message, StopReason, StreamEvent};
use tongs::provider::{Context, Provider, StreamOptions, ToolDef};

use crate::machine::{AgentEvent, ModelCallStatus, StreamDelta};
use crate::model_failure::ModelFailureDiagnostic;
use crate::shell::task_group::CancellationToken;
use crate::shell::{EventClock, EventSink, ModelIdentity};

/// Per-call observability carried as one value so model identity, scope sink,
/// turn, and monotonic clock cannot drift across attempt retries.
pub(super) struct ModelCallObservability<'a> {
    pub(super) turn: usize,
    pub(super) model: &'a ModelIdentity,
    pub(super) clock: &'a dyn EventClock,
    pub(super) events: &'a dyn EventSink,
}

/// Deadline and cancellation authority shared by every retry of one model
/// operation.
#[derive(Clone, Copy)]
pub(super) struct ModelOperationContext<'a> {
    pub(super) connect_timeout: Duration,
    pub(super) idle_timeout: Duration,
    pub(super) cancellation: &'a CancellationToken,
}

/// The shell-level result of one model operation. Cancellation is distinct
/// from provider failure so aborting a run cannot enqueue a model error into a
/// later generation.
pub(super) enum ModelTaskOutcome {
    Responded(AssistantMessage),
    Failed(ModelFailureDiagnostic),
    Cancelled,
}

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

/// Streams one model response and collapses it into a completion. A successful
/// terminal `Done` carries the final assistant message. A terminal `Error`, a
/// defensive `Done + Error`, provider-call failure, timeout, or premature EOF
/// becomes [`ModelTaskOutcome::Failed`] with a typed diagnostic.
pub(super) async fn stream_to_completion(
    provider: &dyn Provider,
    system_prompt: Option<&str>,
    messages: &[Message],
    tool_defs: &[ToolDef],
    stream_options: &StreamOptions,
    operation: ModelOperationContext<'_>,
    observability: ModelCallObservability<'_>,
) -> ModelTaskOutcome {
    let turn = observability.turn;
    let model = observability.model;
    let clock = observability.clock;
    let events = observability.events;
    let context = Context {
        system_prompt: system_prompt.map(std::borrow::Cow::Borrowed),
        messages: std::borrow::Cow::Borrowed(messages),
        tools: std::borrow::Cow::Borrowed(tool_defs),
    };

    // Transient failures (a dropped connection, a 429/5xx, an overloaded
    // provider) are retried with backoff rather than killing the turn — and, in
    // a parallel sub-agent fan-out, a whole run. One flaky socket should not be
    // fatal; this mirrors the resilience the Claude Code CLI gets for free.
    // Retry only happens before any successful terminal message is accepted,
    // so a model turn never double-emits. Terminal provider errors are failed
    // attempts and follow the structured diagnostic's retryability decision.
    let mut attempt = 0u32;
    let retry_config = stream_retry_config();
    let call_id = format!("turn-{turn}");
    loop {
        let started_ms = clock.now_millis();
        events.emit(AgentEvent::ModelCallStarted {
            turn,
            call_id: call_id.clone(),
            attempt,
            provider: model.provider.clone(),
            model: model.model.clone(),
        });
        match stream_one_attempt(
            provider,
            &context,
            stream_options,
            &observability,
            started_ms,
            operation,
        )
        .await
        {
            StreamAttempt::Responded {
                message,
                time_to_first_token_ms,
            } => {
                let duration_ms = clock.now_millis().saturating_sub(started_ms);
                events.emit(AgentEvent::ModelCallFinished {
                    turn,
                    call_id,
                    attempt,
                    status: ModelCallStatus::Succeeded,
                    duration_ms,
                    time_to_first_token_ms,
                    stop_reason: Some(message.stop_reason),
                    usage: message.usage,
                    failure: None,
                });
                return ModelTaskOutcome::Responded(message);
            }
            StreamAttempt::Cancelled {
                time_to_first_token_ms,
            } => {
                let duration_ms = clock.now_millis().saturating_sub(started_ms);
                events.emit(AgentEvent::ModelCallFinished {
                    turn,
                    call_id,
                    attempt,
                    status: ModelCallStatus::Cancelled,
                    duration_ms,
                    time_to_first_token_ms,
                    stop_reason: None,
                    usage: tongs::model::Usage::default(),
                    failure: None,
                });
                return ModelTaskOutcome::Cancelled;
            }
            StreamAttempt::Failed {
                diagnostic,
                stop_reason,
                usage,
                time_to_first_token_ms,
            } => {
                let duration_ms = clock.now_millis().saturating_sub(started_ms);
                events.emit(AgentEvent::ModelCallFinished {
                    turn,
                    call_id: call_id.clone(),
                    attempt,
                    status: ModelCallStatus::Failed,
                    duration_ms,
                    time_to_first_token_ms,
                    stop_reason,
                    usage,
                    failure: Some(diagnostic.clone()),
                });
                let will_retry =
                    diagnostic.retryable() && (attempt as usize) < retry_config.max_retries;
                if will_retry {
                    let delay = retry_config.backoff(attempt as usize);
                    events.emit(AgentEvent::ModelCallRetrying {
                        turn,
                        call_id: call_id.clone(),
                        next_attempt: attempt.saturating_add(1),
                        delay_ms: u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        reason: diagnostic.clone(),
                    });
                    if cancel_or(operation.cancellation, temper_agent_io::sleep_for(delay))
                        .await
                        .is_none()
                    {
                        return ModelTaskOutcome::Cancelled;
                    }
                    attempt = attempt.saturating_add(1);
                    continue;
                }
                return ModelTaskOutcome::Failed(diagnostic);
            }
        }
    }
}

/// Outcome of one streaming attempt.
enum StreamAttempt {
    /// External run cancellation won an in-flight provider wait.
    Cancelled { time_to_first_token_ms: Option<u64> },
    /// The turn produced a successful terminal assistant message.
    Responded {
        message: AssistantMessage,
        time_to_first_token_ms: Option<u64>,
    },
    /// The attempt failed. The diagnostic owns retryability and contains only
    /// typed, bounded facts; terminal stream errors may also retain usage and
    /// stop reason for observability.
    Failed {
        diagnostic: ModelFailureDiagnostic,
        stop_reason: Option<StopReason>,
        usage: tongs::model::Usage,
        time_to_first_token_ms: Option<u64>,
    },
}

/// Runs one streaming attempt with liveness timeouts. Live deltas are forwarded
/// to `events` so observers see tokens/tool-calls in real time; the terminal
/// `Done` carries the assembled message the machine acts on; `Error` carries a
/// sanitized diagnostic and is never treated as a successful response.
async fn stream_one_attempt(
    provider: &dyn Provider,
    context: &Context<'_>,
    stream_options: &StreamOptions,
    observability: &ModelCallObservability<'_>,
    started_ms: u64,
    operation: ModelOperationContext<'_>,
) -> StreamAttempt {
    let model = observability.model;
    let clock = observability.clock;
    let events = observability.events;
    let mut time_to_first_token_ms = None;
    // The connect limit covers the complete time to the first provider event,
    // not merely creation of the pull stream. Providers may return an HTTP body
    // stream as soon as headers arrive and then remain silent forever; treating
    // that first `next()` as idle time would apply the wrong configured limit.
    let first_event = async {
        let mut stream = provider.stream(context, stream_options).await?;
        let event = stream.next().await;
        Ok::<_, tongs::error::Error>((stream, event))
    };
    let Some(connect_result) = cancel_or(
        operation.cancellation,
        temper_agent_io::timeout(operation.connect_timeout, first_event),
    )
    .await
    else {
        return StreamAttempt::Cancelled {
            time_to_first_token_ms,
        };
    };
    let (mut stream, mut next_event) = match connect_result {
        Ok(Ok((stream, Some(event)))) => (stream, Some(event)),
        Ok(Ok((_stream, None))) => {
            return StreamAttempt::Failed {
                diagnostic: ModelFailureDiagnostic::response(
                    model,
                    true,
                    "Model stream ended before a terminal event.",
                ),
                stop_reason: None,
                usage: tongs::model::Usage::default(),
                time_to_first_token_ms,
            };
        }
        Ok(Err(error)) => {
            return StreamAttempt::Failed {
                diagnostic: ModelFailureDiagnostic::from_tongs_error(model, &error),
                stop_reason: None,
                usage: tongs::model::Usage::default(),
                time_to_first_token_ms,
            };
        }
        Err(_) => {
            return StreamAttempt::Failed {
                diagnostic: ModelFailureDiagnostic::timeout(
                    model,
                    "Model connect deadline elapsed.",
                ),
                stop_reason: None,
                usage: tongs::model::Usage::default(),
                time_to_first_token_ms,
            };
        }
    };

    loop {
        let event = if let Some(event) = next_event.take() {
            event
        } else {
            let Some(next_result) = cancel_or(
                operation.cancellation,
                temper_agent_io::timeout(operation.idle_timeout, stream.next()),
            )
            .await
            else {
                return StreamAttempt::Cancelled {
                    time_to_first_token_ms,
                };
            };
            match next_result {
                Ok(Some(event)) => event,
                Ok(None) => {
                    // A clean EOF with no terminal event is usually a dropped
                    // connection mid-stream and retains the existing retry.
                    return StreamAttempt::Failed {
                        diagnostic: ModelFailureDiagnostic::response(
                            model,
                            true,
                            "Model stream ended before a terminal event.",
                        ),
                        stop_reason: None,
                        usage: tongs::model::Usage::default(),
                        time_to_first_token_ms,
                    };
                }
                Err(_) => {
                    return StreamAttempt::Failed {
                        diagnostic: ModelFailureDiagnostic::timeout(
                            model,
                            "Model stream idle deadline elapsed.",
                        ),
                        stop_reason: None,
                        usage: tongs::model::Usage::default(),
                        time_to_first_token_ms,
                    };
                }
            }
        };
        match event {
            Ok(StreamEvent::Done {
                reason,
                mut message,
            }) => {
                // A provider should use StreamEvent::Error for this condition,
                // but fail closed if an adapter emits Done + Error.
                if matches!(reason, StopReason::Error)
                    || matches!(message.stop_reason, StopReason::Error)
                {
                    message.stop_reason = StopReason::Error;
                    return StreamAttempt::Failed {
                        diagnostic: ModelFailureDiagnostic::response(
                            model,
                            false,
                            "Model stream returned an error completion.",
                        ),
                        stop_reason: Some(StopReason::Error),
                        usage: message.usage,
                        time_to_first_token_ms,
                    };
                }
                return StreamAttempt::Responded {
                    message,
                    time_to_first_token_ms,
                };
            }
            Ok(StreamEvent::Error {
                reason,
                error,
                diagnostic,
            }) => {
                return StreamAttempt::Failed {
                    diagnostic: ModelFailureDiagnostic::from_provider(model, &diagnostic),
                    stop_reason: Some(reason),
                    usage: error.usage,
                    time_to_first_token_ms,
                };
            }
            Ok(StreamEvent::TextDelta { delta, .. }) => {
                mark_first_token(&mut time_to_first_token_ms, clock, started_ms);
                events.emit(AgentEvent::StreamDelta(StreamDelta::Text(delta)));
            }
            Ok(StreamEvent::ThinkingDelta { delta, .. }) => {
                mark_first_token(&mut time_to_first_token_ms, clock, started_ms);
                events.emit(AgentEvent::StreamDelta(StreamDelta::Thinking(delta)));
            }
            Ok(StreamEvent::ToolCallEnd { tool_call, .. }) => {
                mark_first_token(&mut time_to_first_token_ms, clock, started_ms);
                events.emit(AgentEvent::StreamDelta(StreamDelta::ToolCall {
                    id: tool_call.id,
                    name: tool_call.name,
                }));
            }
            Ok(_) => {}
            Err(error) => {
                return StreamAttempt::Failed {
                    diagnostic: ModelFailureDiagnostic::from_tongs_error(model, &error),
                    stop_reason: None,
                    usage: tongs::model::Usage::default(),
                    time_to_first_token_ms,
                };
            }
        }
    }
}

async fn cancel_or<F: Future>(cancellation: &CancellationToken, future: F) -> Option<F::Output> {
    match futures::future::select(Box::pin(cancellation.cancelled()), Box::pin(future)).await {
        Either::Left(_) => None,
        Either::Right((output, _)) => Some(output),
    }
}

fn mark_first_token(
    time_to_first_token_ms: &mut Option<u64>,
    clock: &dyn EventClock,
    started_ms: u64,
) {
    if time_to_first_token_ms.is_none() {
        *time_to_first_token_ms = Some(clock.now_millis().saturating_sub(started_ms));
    }
}

#[cfg(test)]
#[path = "streaming_failure_tests.rs"]
mod failure_tests;
#[cfg(test)]
#[path = "streaming_tests.rs"]
mod tests;
