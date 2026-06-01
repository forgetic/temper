//! The real, LLM-driven architect role agent.
//!
//! [`LlmArchitect`] mirrors the deterministic `FakeArchitect`/`ClosingArchitect`
//! pair: the **decision** (triage an intake issue, or reconcile a landed PR)
//! comes from a DeepSeek model, but every **mutation** goes through
//! [`RoleTools`] — the same authority boundary the fakes use.
//!
//! Like the fakes, the architect has a **closing** variant: after reconciling a
//! landed implementation PR it also closes the PR's produced parent issues,
//! unblocking dependents (the `dependency_chain` scenario). Whether to close is a
//! deterministic post-step of the reconcile, not an LLM choice, exactly as in the
//! fake — the model only decides *that* the PR should be reconciled.

use async_trait::async_trait;
use harness_forge::Forge;
use harness_runner::{Agent, AgentError, RoleTools, WorkItem};
use harness_workflow::{ArtifactSource, parse_metadata_block};
use serde::Deserialize;

use crate::common::{build_context, run_or_ignore_stale};
use crate::decision::{DecisionError, run_decision};
use crate::prompts::ARCHITECT_SYSTEM_PROMPT;
use crate::provider::ProviderConfig;

/// The action the LLM chose for an architect work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum ArchitectDecision {
    /// Triage an intake design issue into ready code work.
    TriageToCode,
    /// Reconcile a freshly landed implementation pull request.
    ReconcileLanded,
    /// Do nothing (stale, already handled, or not applicable).
    NoAction,
}

/// A real architect agent: decide with the LLM, act through [`RoleTools`].
///
/// `close_parent_issues` selects the **closing** behavior variant: when set, a
/// successful `reconcile_landed` is followed by closing the PR's produced parent
/// issues (mirroring `ClosingArchitect`).
pub struct LlmArchitect {
    provider: ProviderConfig,
    close_parent_issues: bool,
}

impl LlmArchitect {
    /// Builds the default architect (reconciles, leaves parent issues open).
    pub fn new(provider: ProviderConfig) -> Self {
        Self {
            provider,
            close_parent_issues: false,
        }
    }

    /// Builds the **closing** architect (also closes a merged PR's parent issues).
    pub fn closing(provider: ProviderConfig) -> Self {
        Self {
            provider,
            close_parent_issues: true,
        }
    }

    async fn decide(
        &self,
        item: &WorkItem,
        context: &str,
    ) -> Result<ArchitectDecision, AgentError> {
        match run_decision::<ArchitectDecision>(&self.provider, ARCHITECT_SYSTEM_PROMPT, context)
            .await
        {
            Ok(decision) => Ok(decision),
            Err(DecisionError::Provider(error)) => Err(AgentError::message(error.to_string())),
            Err(error) => {
                eprintln!(
                    "harness-agents: architect LLM decision failed for {:?} on queue '{}', \
                     treating as no-action: {error}",
                    item.target,
                    item.queue.as_str()
                );
                Ok(ArchitectDecision::NoAction)
            }
        }
    }

    /// Closes every parent issue recorded in the landed PR's workflow metadata.
    /// Mirrors `ClosingArchitect::close_produced_parent_issues`.
    async fn close_produced_parent_issues<F: Forge + ?Sized>(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, F>,
    ) -> Result<bool, AgentError> {
        let ArtifactSource::PullRequest { number } = item.target else {
            return Ok(false);
        };
        let Some(pull_request) = tools.get_pull_request(number).await? else {
            return Ok(false);
        };
        let Some(metadata) = parse_metadata_block(&pull_request.body).map_err(|error| {
            AgentError::message(format!("invalid PR workflow metadata: {error}"))
        })?
        else {
            return Ok(false);
        };

        let mut closed = false;
        for parent in metadata.parents {
            if parent.is_same_repo() {
                closed |= tools.close_issue(parent.number).await?;
            }
        }
        Ok(closed)
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for LlmArchitect {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        let context = build_context(item, tools).await?;
        match self.decide(item, &context).await? {
            ArchitectDecision::TriageToCode => {
                run_or_ignore_stale(tools, item.target, "triage_to_code").await
            }
            ArchitectDecision::ReconcileLanded => {
                let reconciled =
                    run_or_ignore_stale(tools, item.target, "reconcile_landed").await?;
                if reconciled && self.close_parent_issues {
                    self.close_produced_parent_issues(item, tools).await?;
                }
                Ok(reconciled)
            }
            ArchitectDecision::NoAction => Ok(false),
        }
    }
}
