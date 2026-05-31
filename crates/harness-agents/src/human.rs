//! The real, LLM-driven human-stakeholder role agent.
//!
//! [`LlmHuman`] mirrors the deterministic `FakeHuman`: the model **decides**
//! whether a design issue escalated for human input should have its human flag
//! cleared, but the **mutation** goes through [`RoleTools`] — the same authority
//! boundary the fake uses.

use async_trait::async_trait;
use harness_forge::Forge;
use harness_runner::{Agent, AgentError, RoleTools, WorkItem};
use serde::Deserialize;

use crate::common::{build_context, run_or_ignore_stale};
use crate::decision::{DecisionError, run_decision};
use crate::prompts::HUMAN_SYSTEM_PROMPT;
use crate::provider::ProviderConfig;

/// The action the LLM chose for a human work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum HumanDecision {
    /// Provide the requested decision and clear the human flag.
    ClearHumanFlag,
    /// Do nothing (stale, already handled, or not applicable).
    NoAction,
}

/// A real human-stakeholder agent: decide with the LLM, act through [`RoleTools`].
pub struct LlmHuman {
    provider: ProviderConfig,
}

impl LlmHuman {
    /// Builds the human-stakeholder agent.
    pub fn new(provider: ProviderConfig) -> Self {
        Self { provider }
    }

    async fn decide(&self, item: &WorkItem, context: &str) -> Result<HumanDecision, AgentError> {
        match run_decision::<HumanDecision>(&self.provider, HUMAN_SYSTEM_PROMPT, context).await {
            Ok(decision) => Ok(decision),
            Err(DecisionError::Provider(error)) => Err(AgentError::message(error.to_string())),
            Err(error) => {
                eprintln!(
                    "harness-agents: human LLM decision failed for {:?} on queue '{}', \
                     treating as no-action: {error}",
                    item.target,
                    item.queue.as_str()
                );
                Ok(HumanDecision::NoAction)
            }
        }
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for LlmHuman {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        let context = build_context(item, tools).await?;
        match self.decide(item, &context).await? {
            HumanDecision::ClearHumanFlag => {
                run_or_ignore_stale(tools, item.target, "clear_human_flag").await
            }
            HumanDecision::NoAction => Ok(false),
        }
    }
}
