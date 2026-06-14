//! The agent loop's imperative shell.
//!
//! [`AgentShell`] implements [`temper_agent_io_engine::Executor`] for
//! [`AgentMachine`](crate::machine::AgentMachine): it performs the two I/O
//! seams the loop has — streaming a model response and executing a tool — by
//! reusing tongs [`Provider`]s and [`Tool`]s, and feeds every result back into
//! the completion queue. Observability events the machine emits as data are
//! forwarded to a sink; the terminal `Finished` request resolves the run's
//! outcome through a oneshot.
//!
//! The shell never calls into the machine; it only spawns I/O and enqueues
//! completions, keeping the loop's logic single-owner and deterministic.

use std::sync::Arc;

use futures::StreamExt;
use temper_agent_io_engine::{CqSender, Executor};
use tongs::model::{AssistantMessage, Message, StopReason, StreamEvent};
use tongs::provider::{Context, Provider, StreamOptions, ToolDef};
use tongs::tools::ToolRegistry;

use crate::machine::{
    AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop, StreamDelta,
};

/// The settled result of a sub-agent run.
#[derive(Clone, Debug)]
pub struct AgentOutcome {
    pub stop: AgentStop,
    pub final_message: AssistantMessage,
    pub messages: Vec<Message>,
}

/// A sink for observability events. The default just drops them; callers that
/// want a live view (a TUI, a log, a transcript recorder) supply their own.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: AgentEvent);
}

/// An [`EventSink`] that discards events.
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn emit(&self, _event: AgentEvent) {}
}

/// An async hook the shell awaits immediately before each model call.
///
/// A model call only starts once the previous turn's tool batch has fully
/// drained (the machine sequences `CallLlm` strictly after the batch), so the
/// hook runs with **no tool in flight** — the natural coherent step boundary
/// for committing and pushing a workspace checkpoint (phase 6b). `turn` is
/// zero-based; the first model call of a run is turn 0 (nothing has happened
/// yet, so checkpoint hooks typically skip it).
///
/// The hook must not fail the run: it returns nothing and implementations
/// swallow their own errors.
#[async_trait::async_trait]
pub trait TurnHook: Send + Sync {
    async fn before_model_call(&self, turn: usize);
}

/// Performs the agent loop's I/O on the skein runtime.
pub struct AgentShell {
    handle: skein::runtime::RuntimeHandle,
    cq: CqSender<AgentCompletion>,
    provider: Arc<dyn Provider>,
    tools: Arc<ToolRegistry>,
    system_prompt: Option<String>,
    tool_defs: Arc<Vec<ToolDef>>,
    stream_options: Arc<StreamOptions>,
    events: Arc<dyn EventSink>,
    /// Awaited before each model call (turn boundary); see [`TurnHook`].
    turn_hook: Option<Arc<dyn TurnHook>>,
    /// Zero-based count of model calls dispatched, for the hook's `turn`.
    turns_started: std::sync::atomic::AtomicUsize,
    /// Resolved once, when the machine emits `Finished`.
    outcome: std::sync::Mutex<Option<temper_agent_io_engine::OneshotSender<AgentOutcome>>>,
}

impl AgentShell {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        handle: skein::runtime::RuntimeHandle,
        cq: CqSender<AgentCompletion>,
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        system_prompt: Option<String>,
        tool_defs: Arc<Vec<ToolDef>>,
        stream_options: Arc<StreamOptions>,
        events: Arc<dyn EventSink>,
        outcome: temper_agent_io_engine::OneshotSender<AgentOutcome>,
    ) -> Self {
        Self {
            handle,
            cq,
            provider,
            tools,
            system_prompt,
            tool_defs,
            stream_options,
            events,
            turn_hook: None,
            turns_started: std::sync::atomic::AtomicUsize::new(0),
            outcome: std::sync::Mutex::new(Some(outcome)),
        }
    }

    /// Installs a [`TurnHook`] awaited before each model call.
    pub fn with_turn_hook(mut self, turn_hook: Arc<dyn TurnHook>) -> Self {
        self.turn_hook = Some(turn_hook);
        self
    }
}

impl Executor<AgentMachine> for AgentShell {
    fn execute(&self, request: AgentRequest) {
        match request {
            AgentRequest::CallLlm { messages } => {
                let provider = Arc::clone(&self.provider);
                let system_prompt = self.system_prompt.clone();
                let tool_defs = Arc::clone(&self.tool_defs);
                let stream_options = Arc::clone(&self.stream_options);
                let events = Arc::clone(&self.events);
                let cq = self.cq.clone();
                let turn_hook = self.turn_hook.clone();
                let turn = self
                    .turns_started
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.handle.spawn(async move {
                    if let Some(hook) = turn_hook {
                        hook.before_model_call(turn).await;
                    }
                    let completion = stream_to_completion(
                        provider.as_ref(),
                        system_prompt.as_deref(),
                        &messages,
                        &tool_defs,
                        &stream_options,
                        events.as_ref(),
                    )
                    .await;
                    // Per-turn token accounting, emitted as soon as the turn's
                    // terminal message lands (both normal and error stops).
                    if let AgentCompletion::LlmResponded(message) = &completion {
                        events.emit(AgentEvent::TurnUsage {
                            turn,
                            usage: message.usage,
                        });
                    }
                    let _ = cq.send(completion);
                });
            }
            AgentRequest::RunTool(call) => {
                let tools = Arc::clone(&self.tools);
                let cq = self.cq.clone();
                self.handle.spawn(async move {
                    let output = match tools.get(&call.name) {
                        Some(tool) => {
                            match tool.execute(&call.id, call.arguments.clone(), None).await {
                                Ok(output) => output,
                                Err(error) => tool_error_output(&format!(
                                    "tool `{}` failed: {error}",
                                    call.name
                                )),
                            }
                        }
                        None => tool_error_output(&format!("unknown tool `{}`", call.name)),
                    };
                    let _ = cq.send(AgentCompletion::ToolFinished {
                        id: call.id,
                        output,
                    });
                });
            }
            AgentRequest::Emit(event) => {
                self.events.emit(event);
            }
            AgentRequest::Finished {
                stop,
                final_message,
                messages,
            } => {
                if let Some(sender) = self.outcome.lock().expect("outcome lock").take() {
                    sender.send(AgentOutcome {
                        stop,
                        final_message,
                        messages,
                    });
                }
            }
        }
    }
}

/// How long to wait for the provider to start responding (connect + TLS +
/// request write + first stream event) before treating the call as stalled.
const STREAM_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How long to wait between stream events once a response has started. A live
/// model turn emits deltas continuously; this only fires when the stream goes
/// silent (a dead socket), so it never cuts off a slow-but-progressing turn.
const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Streams one model response and collapses it into a completion. The terminal
/// `Done` / `Error` stream event carries the final assistant message; a
/// transport-layer failure (the provider call itself erroring, or the stream
/// ending without a terminal event) becomes [`AgentCompletion::LlmFailed`].
async fn stream_to_completion(
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
    loop {
        match stream_one_attempt(provider, &context, stream_options, events).await {
            StreamAttempt::Responded(message) => {
                return AgentCompletion::LlmResponded(message);
            }
            StreamAttempt::Failed { reason, retryable } => {
                let will_retry = retryable && attempt < MAX_STREAM_RETRIES;
                events.emit(AgentEvent::ModelCallFailed {
                    reason: reason.clone(),
                    will_retry,
                });
                if will_retry {
                    let backoff = (STREAM_RETRY_BACKOFF * (1u32 << attempt.min(5)))
                        .min(STREAM_RETRY_BACKOFF_MAX);
                    temper_agent_io_engine::sleep_for(backoff).await;
                    attempt += 1;
                    continue;
                }
                return AgentCompletion::LlmFailed(reason);
            }
        }
    }
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
    let mut stream = match temper_agent_io_engine::timeout(
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
        let event = match temper_agent_io_engine::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await
        {
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

/// Builds an error [`ToolOutput`] carrying `message` as text.
fn tool_error_output(message: &str) -> tongs::tools::ToolOutput {
    tongs::tools::ToolOutput {
        content: vec![tongs::model::ContentBlock::Text(
            tongs::model::TextContent {
                text: message.to_string(),
                text_signature: None,
            },
        )],
        details: None,
        is_error: true,
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
