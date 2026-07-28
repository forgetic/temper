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
use crate::model_failure::{ModelFailureBoundary, ModelFailureDiagnostic, ModelFailureEventKind};
use crate::run::ModelRetryLimits;
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
    pub(super) retry: ModelRetryLimits,
    pub(super) retry_runtime: &'a dyn StreamRetryRuntime,
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

#[cfg(test)]
const MAX_STREAM_RETRIES: usize = 6;

/// Compatibility name retained for hermetic test support.
#[cfg(feature = "test-support")]
pub type StreamRetryConfig = ModelRetryLimits;

#[async_trait::async_trait]
pub(super) trait StreamRetryRuntime: Send + Sync {
    async fn sleep(&self, delay: Duration);
    /// Returns a sample in `0..=10_000` for bounded symmetric jitter.
    fn jitter_sample(&self, call_id: &str, next_attempt: u32) -> u32;
}

pub(super) struct SystemStreamRetryRuntime;

#[async_trait::async_trait]
impl StreamRetryRuntime for SystemStreamRetryRuntime {
    async fn sleep(&self, delay: Duration) {
        temper_agent_io::sleep_for(delay).await;
    }

    fn jitter_sample(&self, call_id: &str, next_attempt: u32) -> u32 {
        // No ambient RNG is needed: mix a monotonic timestamp with stable call
        // identity. Tests inject their own policy and production calls still
        // decorrelate concurrent retry waves.
        let mut hash = temper_agent_io::engine_now().as_nanos() as u64;
        for byte in call_id.bytes().chain(next_attempt.to_le_bytes()) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        (hash % 10_001) as u32
    }
}

pub(super) static SYSTEM_STREAM_RETRY_RUNTIME: SystemStreamRetryRuntime = SystemStreamRetryRuntime;

impl ModelRetryLimits {
    fn delay(self, retry_index: u32, sample: u32) -> Duration {
        let exponent = retry_index.min(31);
        let nominal = self
            .base_delay
            .saturating_mul(1u32 << exponent)
            .min(self.max_delay);
        let percent = u32::from(self.jitter_percent.min(100));
        if percent == 0 {
            return nominal;
        }
        let nanos = nominal.as_nanos();
        let lower = nanos.saturating_mul(u128::from(100 - percent)) / 100;
        let width = nanos.saturating_mul(u128::from(percent.saturating_mul(2))) / 100;
        let selected =
            lower.saturating_add(width.saturating_mul(u128::from(sample.min(10_000))) / 10_000);
        Duration::from_nanos(u64::try_from(selected).unwrap_or(u64::MAX)).min(self.max_delay)
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

fn stream_retry_config(configured: ModelRetryLimits) -> ModelRetryLimits {
    #[cfg(feature = "test-support")]
    {
        if let Some(config) = *STREAM_RETRY_CONFIG_OVERRIDE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            return config;
        }
    }
    configured
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
    let retry_config = stream_retry_config(operation.retry);
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
                let will_retry = diagnostic.eligible_for_turn_retry()
                    && attempt.saturating_add(1) < retry_config.max_attempts;
                if will_retry {
                    let next_attempt = attempt.saturating_add(1);
                    let delay = retry_config.delay(
                        attempt,
                        operation
                            .retry_runtime
                            .jitter_sample(&call_id, next_attempt),
                    );
                    events.emit(AgentEvent::ModelCallRetrying {
                        turn,
                        call_id: call_id.clone(),
                        next_attempt,
                        delay_ms: u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        reason: diagnostic.clone(),
                    });
                    if cancel_or(operation.cancellation, operation.retry_runtime.sleep(delay))
                        .await
                        .is_none()
                    {
                        return ModelTaskOutcome::Cancelled;
                    }
                    attempt = next_attempt;
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
                    ModelFailureBoundary::Sse,
                    ModelFailureEventKind::StreamEof,
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
                    ModelFailureEventKind::ConnectTimeout,
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
                            ModelFailureBoundary::Sse,
                            ModelFailureEventKind::StreamEof,
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
                            ModelFailureEventKind::StreamIdleTimeout,
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
                            ModelFailureBoundary::Sse,
                            ModelFailureEventKind::ErrorCompletion,
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
                    diagnostic: ModelFailureDiagnostic::from_stream_event(model, &diagnostic),
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
