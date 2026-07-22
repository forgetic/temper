//! Sub-agents as tools.
//!
//! A **sub-agent** is exposed to a parent agent as just another [`tongs::tools::Tool`]:
//! when the parent model calls it, [`SubAgentTool::execute`] runs a nested
//! [`run_sub_agent`](crate::run::run_sub_agent) with a task string drawn from the
//! tool arguments, and returns the sub-agent's final message text as the tool
//! result. Because it is an ordinary tool, it composes with everything the agent
//! loop already does:
//!
//! - the parent's [`AgentMachine`](crate::machine::AgentMachine) dispatches it
//!   like any tool call;
//! - it participates in **effect-aware batching** — declare
//!   [`ToolEffects::read`] for a read-only investigator and a parent can fan out
//!   several sub-agents *concurrently* in one batch; declare a write/process
//!   effect to serialize one that mutates a shared workspace;
//! - its result flows back into the parent's conversation as a `ToolFinished`
//!   completion, with no special-casing in the loop.
//!
//! This is the composition the design aimed for: concurrency lives *inside* one
//! agent (fan-out of sub-agents / parallel tools), not across worker jobs.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};

use async_trait::async_trait;
use skein::runtime::RuntimeHandle;
use tongs::error::{Error, Result};
use tongs::model::ContentBlock;
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

use crate::run::{
    SubAgent, SubAgentControl, SubAgentError, run_sub_agent_controllable_with_observability,
};
use crate::shell::{AgentOutcome, EventSink, ModelIdentity, NullEventSink, RunObservability};

/// Builds the nested [`SubAgent`] to run for one invocation, given the task
/// string the parent model supplied. Called fresh per tool call so each
/// invocation gets its own tools/workspace/budget.
pub type SubAgentFactory = Arc<dyn Fn(String) -> SubAgent + Send + Sync>;

/// Builds a fresh observer for one nested invocation. Unlike a static event
/// sink, this factory can mint a unique scope identity for concurrent calls.
pub type SubAgentObserverFactory = Arc<dyn Fn() -> RunObservability + Send + Sync>;

/// A [`Tool`] that runs a nested sub-agent.
pub struct SubAgentTool {
    name: String,
    description: String,
    effects: ToolEffects,
    /// JSON-schema for the tool input. Defaults to `{ task: string }`.
    parameters: serde_json::Value,
    factory: SubAgentFactory,
    /// Observability sink forwarded to every nested run (token usage, tool
    /// starts). `None` keeps nested runs silent.
    events: Option<Arc<dyn EventSink>>,
    /// Preferred scope-aware observer factory. Called once per invocation so
    /// parallel sub-agents never share scope identity or in-flight state.
    observer_factory: Option<SubAgentObserverFactory>,
    /// Kept in the public constructor for compatibility. Nested runs now own a
    /// dedicated runtime so cancellation and recursive tool cleanup never run
    /// on the parent event-loop thread.
    _handle: RuntimeHandle,
}

impl SubAgentTool {
    /// Build a sub-agent tool. `name`/`description` are what the parent model
    /// sees (make the description say *when* to delegate to this sub-agent).
    /// `effects` governs batching — [`ToolEffects::read`] for a read-only
    /// sub-agent that is safe to run in parallel with siblings. `factory`
    /// assembles the nested [`SubAgent`] from the task string. `handle` remains
    /// part of the compatibility surface; each invocation is driven on its own
    /// joined runtime so parent cancellation can be requested without sharing
    /// the parent event-loop thread.
    pub fn new(
        handle: RuntimeHandle,
        name: impl Into<String>,
        description: impl Into<String>,
        effects: ToolEffects,
        factory: SubAgentFactory,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            effects,
            parameters: default_task_schema(),
            factory,
            events: None,
            observer_factory: None,
            _handle: handle,
        }
    }

    /// Override the input JSON-schema (default `{ task: string }`).
    pub fn with_parameters(mut self, parameters: serde_json::Value) -> Self {
        self.parameters = parameters;
        self
    }

    /// Forward nested runs' observability events (token usage, tool starts)
    /// to `events`.
    pub fn with_events(mut self, events: Arc<dyn EventSink>) -> Self {
        self.events = Some(events);
        self
    }

    /// Install a scope-aware observer factory. It takes precedence over the
    /// legacy static sink configured by [`Self::with_events`].
    pub fn with_observer_factory(mut self, factory: SubAgentObserverFactory) -> Self {
        self.observer_factory = Some(factory);
        self
    }
}

#[async_trait]
impl Tool for SubAgentTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    fn effects(&self) -> ToolEffects {
        self.effects
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> Result<ToolOutput> {
        let task = input
            .get("task")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                Error::tool(
                    self.name.clone(),
                    "sub-agent input must include a string `task`",
                )
            })?
            .to_string();

        let sub_agent = (self.factory)(task);
        let observability = if let Some(factory) = &self.observer_factory {
            factory()
        } else {
            let events: Arc<dyn EventSink> = self
                .events
                .as_ref()
                .map(Arc::clone)
                .unwrap_or_else(|| Arc::new(NullEventSink));
            let model = ModelIdentity::new(sub_agent.provider.api(), "unknown");
            RunObservability::new(events, model)
        };
        let outcome = ManagedSubAgentRun::spawn(sub_agent, observability)
            .map_err(|error| Error::tool(self.name.clone(), error.to_string()))?
            .await
            .map_err(|error| Error::tool(self.name.clone(), error.to_string()))?;

        // The sub-agent's product is ordinary output only after a completed
        // stop. Every other terminal reason is returned as a failed tool result
        // so parseable-looking text cannot masquerade as successful nested
        // output.
        let text = collect_text(&outcome.final_message.content);
        let is_error = sub_agent_stop_is_error(outcome.stop);
        Ok(ToolOutput {
            content: vec![ContentBlock::Text(tongs::model::TextContent {
                text,
                text_signature: None,
            })],
            details: Some(serde_json::json!({ "sub_agent_stop": format!("{:?}", outcome.stop) })),
            is_error,
        })
    }
}

fn sub_agent_stop_is_error(stop: crate::machine::AgentStop) -> bool {
    match stop {
        crate::machine::AgentStop::Completed => false,
        crate::machine::AgentStop::ModelError
        | crate::machine::AgentStop::Aborted
        | crate::machine::AgentStop::BudgetExhausted => true,
    }
}

struct ManagedSubAgentState {
    control: Option<SubAgentControl>,
    result: Option<std::result::Result<AgentOutcome, SubAgentError>>,
    waker: Option<Waker>,
}

/// Nested run on a dedicated runtime. Drop reaches every published
/// `SubAgentControl` and aborts the nested machine, while the owner thread keeps
/// driving model/tool wrappers to quiescence independently of the parent loop.
struct ManagedSubAgentRun {
    state: Arc<Mutex<ManagedSubAgentState>>,
    cancelled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ManagedSubAgentRun {
    fn spawn(sub_agent: SubAgent, observability: RunObservability) -> std::io::Result<Self> {
        let state = Arc::new(Mutex::new(ManagedSubAgentState {
            control: None,
            result: None,
            waker: None,
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let control_state = Arc::clone(&state);
        let thread_cancelled = Arc::clone(&cancelled);
        let thread = thread::Builder::new()
            .name("temper-nested-agent".to_string())
            .spawn(move || {
                let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
                    let (control, run) = run_sub_agent_controllable_with_observability(
                        handle,
                        sub_agent,
                        observability,
                        None,
                        None,
                    )?;
                    {
                        let mut state = control_state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        state.control = Some(control.clone());
                    }
                    if thread_cancelled.load(Ordering::Acquire) {
                        control.abort();
                    }
                    let result = run.await;
                    control_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .control
                        .take();
                    result
                });
                let waker = {
                    let mut state = thread_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.result = Some(result);
                    state.waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            })?;
        Ok(Self {
            state,
            cancelled,
            thread: Some(thread),
        })
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Future for ManagedSubAgentRun {
    type Output = std::result::Result<AgentOutcome, SubAgentError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.result.is_none()
                && !state
                    .waker
                    .as_ref()
                    .is_some_and(|waker| waker.will_wake(cx.waker()))
            {
                state.waker = Some(cx.waker().clone());
            }
            state.result.take()
        };
        match result {
            Some(result) => {
                self.join();
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for ManagedSubAgentRun {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(control) = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .control
            .clone()
        {
            control.abort();
        }
        // The nested runtime is already a dedicated owner. Detach its join
        // handle so parent cancellation cannot synchronously recurse through
        // nested tool cleanup on the standalone event-loop thread.
        let _ = self.thread.take();
    }
}

/// The default `{ task: string }` input schema.
fn default_task_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "task": {
                "type": "string",
                "description": "The task for the sub-agent to perform."
            }
        },
        "required": ["task"]
    })
}

/// Concatenate an assistant message's text blocks.
fn collect_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tongs::provider::{Context as ProviderContext, EventStream, Provider, StreamOptions};
    use tongs::tools::ToolRegistry;

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    struct HungProvider {
        started: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl Provider for HungProvider {
        fn api(&self) -> &str {
            "hung-nested"
        }

        async fn stream(
            &self,
            _context: &ProviderContext<'_>,
            _options: &StreamOptions,
        ) -> tongs::Result<EventStream> {
            let _drop = DropFlag(Arc::clone(&self.dropped));
            self.started.store(true, Ordering::Release);
            futures::future::pending().await
        }
    }

    #[test]
    fn every_non_completed_nested_stop_is_an_error() {
        assert!(!sub_agent_stop_is_error(
            crate::machine::AgentStop::Completed
        ));
        for stop in [
            crate::machine::AgentStop::ModelError,
            crate::machine::AgentStop::Aborted,
            crate::machine::AgentStop::BudgetExhausted,
        ] {
            assert!(sub_agent_stop_is_error(stop), "{stop:?} must be an error");
        }
    }

    #[test]
    fn dropping_nested_run_aborts_every_control_on_its_dedicated_owner() {
        let started = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let sub_agent = SubAgent {
            system_prompt: None,
            user_message: "wait".to_string(),
            tools: ToolRegistry::new(),
            max_iterations: 1,
            operation_limits: crate::run::AgentOperationLimits::default(),
            provider: Arc::new(HungProvider {
                started: Arc::clone(&started),
                dropped: Arc::clone(&dropped),
            }),
            stream_options: StreamOptions::default(),
        };
        let run = ManagedSubAgentRun::spawn(
            sub_agent,
            RunObservability::new(Arc::new(NullEventSink), ModelIdentity::new("test", "hung")),
        )
        .expect("start nested run owner");
        let deadline = Instant::now() + Duration::from_millis(500);
        while !started.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            started.load(Ordering::Acquire),
            "nested model did not start"
        );

        let cancellation_started = Instant::now();
        drop(run);
        while !dropped.load(Ordering::Acquire)
            && cancellation_started.elapsed() < Duration::from_millis(500)
        {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(dropped.load(Ordering::Acquire));
        assert!(cancellation_started.elapsed() < Duration::from_millis(500));
    }
}
