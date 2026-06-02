//! The real, LLM-driven owner role agent.
//!
//! [`LlmOwner`] mirrors the deterministic `FakeOwner`: the model **decides**
//! which owner action applies (alignment review, merge approval, or escalating a
//! design issue for human input), but every **mutation** goes through
//! [`RoleTools`] — the same authority boundary the fake uses.

use async_trait::async_trait;
use serde::Deserialize;
use temper_forge::Forge;
use temper_runner::{Agent, AgentError, RoleTools, WorkItem};

use temper_agents::decision::{run_decision, DecisionError};
use temper_agents::ProviderConfig;

use super::common::{build_context, run_or_ignore_stale};
use super::prompts::OWNER_SYSTEM_PROMPT;

/// The action the LLM chose for an owner work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum OwnerDecision {
    /// Confirm an implementation PR aligns with project direction.
    ReviewAlignment,
    /// Approve an implementation PR for merge.
    ApproveMerge,
    /// Escalate a design issue for human input.
    RequestHumanInput,
    /// Do nothing (stale, already handled, or not applicable).
    NoAction,
}

/// A real owner agent: decide with the LLM, act through [`RoleTools`].
pub struct LlmOwner {
    provider: ProviderConfig,
}

impl LlmOwner {
    /// Builds the owner agent.
    pub fn new(provider: ProviderConfig) -> Self {
        Self { provider }
    }

    async fn decide(&self, item: &WorkItem, context: &str) -> Result<OwnerDecision, AgentError> {
        match run_decision::<OwnerDecision>(&self.provider, OWNER_SYSTEM_PROMPT, context).await {
            Ok(decision) => Ok(decision),
            Err(DecisionError::Provider(error)) => Err(AgentError::message(error.to_string())),
            Err(error) => {
                eprintln!(
                    "temper-agents: owner LLM decision failed for {:?} on queue '{}', \
                     treating as no-action: {error}",
                    item.target,
                    item.queue.as_str()
                );
                Ok(OwnerDecision::NoAction)
            }
        }
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for LlmOwner {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        let context = build_context(item, tools).await?;
        let transition = match self.decide(item, &context).await? {
            OwnerDecision::ReviewAlignment => "review_alignment",
            OwnerDecision::ApproveMerge => "approve_merge",
            OwnerDecision::RequestHumanInput => "request_human_input",
            OwnerDecision::NoAction => return Ok(false),
        };
        run_or_ignore_stale(tools, item.target, transition).await
    }
}
