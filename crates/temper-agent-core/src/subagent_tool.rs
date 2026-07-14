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

use std::sync::Arc;

use async_trait::async_trait;
use skein::runtime::RuntimeHandle;
use tongs::error::{Error, Result};
use tongs::model::ContentBlock;
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

use crate::run::{
    SubAgent, run_sub_agent, run_sub_agent_controllable_with_observability,
    run_sub_agent_with_events,
};
use crate::shell::{EventSink, RunObservability};

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
    /// Runtime spawn capability, forwarded to each nested run. Passed
    /// explicitly (no ambient handle lookup).
    handle: RuntimeHandle,
}

impl SubAgentTool {
    /// Build a sub-agent tool. `name`/`description` are what the parent model
    /// sees (make the description say *when* to delegate to this sub-agent).
    /// `effects` governs batching — [`ToolEffects::read`] for a read-only
    /// sub-agent that is safe to run in parallel with siblings. `factory`
    /// assembles the nested [`SubAgent`] from the task string. `handle` is the
    /// runtime spawn capability each nested run needs.
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
            handle,
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
        let outcome = if let Some(factory) = &self.observer_factory {
            let (_control, run) = run_sub_agent_controllable_with_observability(
                self.handle.clone(),
                sub_agent,
                factory(),
                None,
                None,
            )
            .map_err(|error| Error::tool(self.name.clone(), error.to_string()))?;
            run.await
        } else {
            match &self.events {
                Some(events) => {
                    run_sub_agent_with_events(self.handle.clone(), sub_agent, Arc::clone(events))
                        .await
                }
                None => run_sub_agent(self.handle.clone(), sub_agent).await,
            }
        }
        .map_err(|error| Error::tool(self.name.clone(), error.to_string()))?;

        // The sub-agent's product is the text of its final message. A
        // sub-agent that ended in an error stop reports a tool error so the
        // parent model can react.
        let text = collect_text(&outcome.final_message.content);
        let is_error = matches!(outcome.stop, crate::machine::AgentStop::ModelError);
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
