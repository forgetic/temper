//! Manifest-driven LLM workflow role agent.
//!
//! [`LlmRoleAgent`] is the generic replacement path for hard-coded role prompt
//! constants and role-specific decision enums: it owns a compiled
//! [`RoleManifest`], asks a decision engine for one `{ action, reason }` JSON
//! decision using the manifest's rendered prompt, validates that action against
//! the manifest's tool list, and runs only the matching workflow transition
//! through [`RoleTools`].

use std::sync::Arc;

use async_trait::async_trait;
use harness_forge::Forge;
use harness_runner::{Agent, AgentError, BoundExternalTool, RoleTools, WorkItem};
use harness_workflow::{RoleManifest, TransitionId};
use serde::Deserialize;

use crate::common::{build_context, run_or_ignore_stale};
use crate::decision::{DecisionError, run_decision};
use crate::provider::ProviderConfig;

const NO_ACTION: &str = "no_action";
const EXTERNAL_TOOL_SECTION: &str = "User-declared external tools";

/// Generic workflow-role decision returned by a model or injected test seam.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct RoleDecision {
    /// One manifest tool name, or [`no_action`](NO_ACTION).
    pub action: String,
    /// Short rationale for logs and operator debugging. The generic adapter does
    /// not use this to grant authority.
    #[serde(default)]
    pub reason: String,
}

impl RoleDecision {
    /// Builds a decision that deliberately makes no workflow mutation.
    pub fn no_action(reason: impl Into<String>) -> Self {
        Self {
            action: NO_ACTION.to_string(),
            reason: reason.into(),
        }
    }

    /// Builds a decision for an action name.
    pub fn action(action: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            reason: reason.into(),
        }
    }
}

/// Mockable seam for obtaining one generic role decision.
#[async_trait]
pub trait RoleDecisionEngine: Send + Sync {
    /// Decide from the system prompt and user context the adapter constructed.
    async fn decide(
        &self,
        system_prompt: &str,
        user_context: &str,
    ) -> Result<RoleDecision, DecisionError>;
}

/// Provider-backed decision engine that runs the real `pi` SDK path.
pub struct ProviderRoleDecisionEngine {
    provider: ProviderConfig,
}

impl ProviderRoleDecisionEngine {
    /// Builds a decision engine backed by the configured LLM provider.
    pub fn new(provider: ProviderConfig) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl RoleDecisionEngine for ProviderRoleDecisionEngine {
    async fn decide(
        &self,
        system_prompt: &str,
        user_context: &str,
    ) -> Result<RoleDecision, DecisionError> {
        run_decision::<RoleDecision>(&self.provider, system_prompt, user_context).await
    }
}

/// Generic LLM agent for one compiled workflow role.
pub struct LlmRoleAgent {
    manifest: RoleManifest,
    bound_external_tools: Vec<BoundExternalTool>,
    decision_engine: Arc<dyn RoleDecisionEngine>,
}

impl LlmRoleAgent {
    /// Builds a provider-backed agent for `manifest`.
    pub fn new(manifest: RoleManifest, provider: ProviderConfig) -> Self {
        Self::with_bound_external_tools(manifest, provider, Vec::new())
    }

    /// Builds a provider-backed agent with external tools validated by the runner.
    pub fn with_bound_external_tools(
        manifest: RoleManifest,
        provider: ProviderConfig,
        bound_external_tools: Vec<BoundExternalTool>,
    ) -> Self {
        Self::with_decision_engine_and_external_tools(
            manifest,
            Arc::new(ProviderRoleDecisionEngine::new(provider)) as Arc<dyn RoleDecisionEngine>,
            bound_external_tools,
        )
    }

    /// Builds an agent with an injected decision engine, for hermetic tests or
    /// alternate providers.
    pub fn with_decision_engine(
        manifest: RoleManifest,
        decision_engine: Arc<dyn RoleDecisionEngine>,
    ) -> Self {
        Self::with_decision_engine_and_external_tools(manifest, decision_engine, Vec::new())
    }

    /// Builds an agent with an injected decision engine and runner-bound tools.
    pub fn with_decision_engine_and_external_tools(
        manifest: RoleManifest,
        decision_engine: Arc<dyn RoleDecisionEngine>,
        bound_external_tools: Vec<BoundExternalTool>,
    ) -> Self {
        let bound_external_tools = declared_bound_tools(&manifest, bound_external_tools);
        Self {
            manifest,
            bound_external_tools,
            decision_engine,
        }
    }

    /// Returns the compiled role manifest this agent enforces.
    pub fn manifest(&self) -> &RoleManifest {
        &self.manifest
    }

    /// Returns the declared-and-bound external tools visible to the model.
    pub fn bound_external_tools(&self) -> &[BoundExternalTool] {
        &self.bound_external_tools
    }

    async fn decide(&self, item: &WorkItem, context: &str) -> Result<RoleDecision, AgentError> {
        let system_prompt = self.runtime_system_prompt();
        match self.decision_engine.decide(&system_prompt, context).await {
            Ok(decision) => Ok(decision),
            Err(DecisionError::Provider(error)) => Err(AgentError::message(error.to_string())),
            Err(error) => {
                eprintln!(
                    "harness-agents: LLM decision failed for role '{}' on {:?} queue '{}', treating as no-action: {error}",
                    self.manifest.id,
                    item.target,
                    item.queue.as_str()
                );
                Ok(RoleDecision::no_action("decision failed"))
            }
        }
    }

    fn transition_for_action(&self, action: &str) -> Option<&TransitionId> {
        self.manifest
            .tools
            .iter()
            .find(|tool| tool.name == action)
            .map(|tool| &tool.transition)
    }

    fn runtime_system_prompt(&self) -> String {
        if self.manifest.external_tools.is_empty() {
            return self.manifest.prompt.render();
        }
        let mut prompt = self.manifest.prompt.clone();
        if let Some(section) = prompt.section_mut(EXTERNAL_TOOL_SECTION) {
            section.lines = runtime_external_tool_lines(&self.bound_external_tools);
        }
        prompt.render()
    }

    fn user_context(&self, work_item_context: &str) -> String {
        let work_item = serde_json::from_str::<serde_json::Value>(work_item_context)
            .unwrap_or_else(|_| serde_json::Value::String(work_item_context.to_string()));
        let allowed_actions = std::iter::once(NO_ACTION.to_string())
            .chain(self.manifest.tools.iter().map(|tool| tool.name.clone()))
            .collect::<Vec<_>>();
        let authorized_actions = self
            .manifest
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "action": tool.name,
                    "transition": tool.transition.as_str(),
                    "artifact": tool.artifact.as_str(),
                    "requires_gates": tool
                        .requires_gates
                        .iter()
                        .map(|gate| gate.as_str())
                        .collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        let available_external_tools = self
            .bound_external_tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "id": tool.id.as_str(),
                    "provider": tool.provider.as_str(),
                    "description": tool.description.as_str(),
                    "required": tool.required,
                    "constraints": &tool.constraints,
                    "guidance": tool.guidance.as_deref(),
                })
            })
            .collect::<Vec<_>>();
        let context = serde_json::json!({
            "work_item": work_item,
            "allowed_actions": allowed_actions,
            "authorized_actions": authorized_actions,
            "available_external_tools": available_external_tools,
        });
        serde_json::to_string_pretty(&context).unwrap_or_else(|_| context.to_string())
    }
}

fn declared_bound_tools(
    manifest: &RoleManifest,
    bound_external_tools: Vec<BoundExternalTool>,
) -> Vec<BoundExternalTool> {
    manifest
        .external_tools
        .iter()
        .filter_map(|declared| {
            bound_external_tools
                .iter()
                .find(|tool| tool.id == declared.id)
                .cloned()
        })
        .collect()
}

fn runtime_external_tool_lines(tools: &[BoundExternalTool]) -> Vec<String> {
    let mut lines = vec![
        "Only the external tools listed in this section are bound and available for this run."
            .to_string(),
        "Declared tools not listed here are unavailable; do not claim to use them.".to_string(),
        "External tools do not grant workflow or Forge mutation authority beyond the authorized workflow actions above.".to_string(),
    ];
    if tools.is_empty() {
        lines.push("(no external tools are bound for this run)".to_string());
    } else {
        for tool in tools {
            lines.push(format!(
                "{} via {}: {}",
                tool.id, tool.provider, tool.description
            ));
            if !tool.constraints.is_empty() {
                lines.push(format!(
                    "{} constraints: {}",
                    tool.id,
                    tool.constraints.join("; ")
                ));
            }
            if let Some(guidance) = &tool.guidance {
                lines.push(format!("{} guidance: {guidance}", tool.id));
            }
        }
    }
    lines
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for LlmRoleAgent {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        let work_item_context = build_context(item, tools).await?;
        let context = self.user_context(&work_item_context);
        let decision = self.decide(item, &context).await?;

        if decision.action == NO_ACTION {
            return Ok(false);
        }

        let Some(transition) = self.transition_for_action(&decision.action) else {
            eprintln!(
                "harness-agents: role '{}' returned unauthorized action '{}', treating as no-action",
                self.manifest.id, decision.action
            );
            return Ok(false);
        };

        run_or_ignore_stale(tools, item.target, transition.as_str()).await
    }
}

#[cfg(test)]
#[path = "role_tests.rs"]
mod role_tests;
