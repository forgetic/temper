//! The real, LLM-driven reviewer role agent.
//!
//! [`LlmReviewer`] mirrors the deterministic `FakeReviewer` /
//! `RequestChangesThenApproveReviewer` pair: the model **decides** which review
//! action to take, but the review is submitted only through [`RoleTools`] — the
//! same authority boundary the fakes use.
//!
//! ## The request-changes-then-approve variant
//!
//! The default reviewer approves on the first pass. The variant must request
//! changes first, then approve on a later pass — the workflow gives no per-PR
//! label that distinguishes the two passes, so (exactly as the fake does) the
//! adapter keeps a per-PR success counter and tells the model which pass this is
//! through a `review_instruction` field in the context. The model still makes the
//! call; the counter only steers *which* instruction it is given, and advances
//! only when a `request_changes` review actually lands (so a stale first attempt
//! does not skip the request-changes step). This keeps the LLM in the decision
//! seat while matching the fake's two-step behavior deterministically.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::Deserialize;
use temper_forge::{Forge, ItemNumber};
use temper_runner::{Agent, AgentError, RoleTools, WorkItem};
use temper_workflow::ArtifactSource;

use temper_agents::decision::{run_decision, DecisionError};
use temper_agents::ProviderConfig;

use super::common::stale_execution;
use super::prompts::REVIEWER_SYSTEM_PROMPT;

/// The action the LLM chose for a reviewer work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum ReviewerDecision {
    /// Approve the implementation pull request.
    ApproveReview,
    /// Ask the engineer to revise the pull request.
    RequestChanges,
    /// Do nothing (stale, already handled, or not applicable).
    NoAction,
}

/// Which review behavior a reviewer agent performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewMode {
    /// Approve on the first review.
    Approve,
    /// Request changes first, then approve on a later pass.
    RequestChangesThenApprove,
}

/// A real reviewer agent: decide with the LLM, submit through [`RoleTools`].
pub struct LlmReviewer {
    provider: ProviderConfig,
    mode: ReviewMode,
    /// Per-PR count of **landed** `request_changes` reviews, used only by the
    /// request-changes-then-approve variant to pick the next instruction.
    visits: Mutex<BTreeMap<ItemNumber, u64>>,
}

impl LlmReviewer {
    /// Builds the default reviewer (approves on the first review).
    pub fn new(provider: ProviderConfig) -> Self {
        Self {
            provider,
            mode: ReviewMode::Approve,
            visits: Mutex::new(BTreeMap::new()),
        }
    }

    /// Builds the request-changes-then-approve reviewer variant.
    pub fn request_changes_then_approve(provider: ProviderConfig) -> Self {
        Self {
            provider,
            mode: ReviewMode::RequestChangesThenApprove,
            visits: Mutex::new(BTreeMap::new()),
        }
    }

    /// The review instruction this pass should follow for `number`, given the
    /// mode and how many request-changes reviews have already landed.
    fn instruction_for(&self, number: ItemNumber) -> &'static str {
        match self.mode {
            ReviewMode::Approve => "Approve this pull request.",
            ReviewMode::RequestChangesThenApprove => {
                let visits = self
                    .visits
                    .lock()
                    .expect("reviewer visit mutex is poisoned");
                if visits.get(&number).copied().unwrap_or(0) == 0 {
                    "Request changes on this pull request; it needs revision before it can merge."
                } else {
                    "You previously requested changes and they have been addressed. Approve this \
                     pull request now."
                }
            }
        }
    }

    /// Records that a `request_changes` review **landed** for `number`. Called
    /// only on success so a stale first attempt does not skip the step.
    fn record_request_changes(&self, number: ItemNumber) {
        let mut visits = self
            .visits
            .lock()
            .expect("reviewer visit mutex is poisoned");
        let visit = visits.entry(number).or_insert(0);
        *visit = visit.saturating_add(1);
    }

    async fn decide(&self, item: &WorkItem, context: &str) -> Result<ReviewerDecision, AgentError> {
        match run_decision::<ReviewerDecision>(&self.provider, REVIEWER_SYSTEM_PROMPT, context)
            .await
        {
            Ok(decision) => Ok(decision),
            Err(DecisionError::Provider(error)) => Err(AgentError::message(error.to_string())),
            Err(error) => {
                eprintln!(
                    "temper-agents: reviewer LLM decision failed for {:?} on queue '{}', \
                     treating as no-action: {error}",
                    item.target,
                    item.queue.as_str()
                );
                Ok(ReviewerDecision::NoAction)
            }
        }
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for LlmReviewer {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        if item.queue.as_str() != "pr_needs_review" || item.kind.as_str() != "implementation_pr" {
            return Ok(false);
        }
        let ArtifactSource::PullRequest { number } = item.target else {
            return Ok(false);
        };

        let context = build_reviewer_context(item, tools, self.instruction_for(number)).await?;
        let decision = self.decide(item, &context).await?;

        let transition = match decision {
            ReviewerDecision::ApproveReview => "approve_review",
            ReviewerDecision::RequestChanges => "request_changes",
            ReviewerDecision::NoAction => return Ok(false),
        };

        match tools
            .run(item.target, &temper_workflow::TransitionId::new(transition))
            .await
        {
            Ok(_) => {
                if transition == "request_changes" {
                    self.record_request_changes(number);
                }
                Ok(true)
            }
            Err(error) if stale_execution(&error) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

/// The reviewer's context: the shared work-item JSON plus the `review_instruction`
/// that tells the model what this pass should do.
async fn build_reviewer_context<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
    instruction: &str,
) -> Result<String, AgentError> {
    let pull_request = match item.target {
        ArtifactSource::PullRequest { number } => tools.get_pull_request(number).await?,
        ArtifactSource::Issue { .. } => None,
    };
    let artifact = pull_request.map(|pr| {
        serde_json::json!({
            "type": "pull_request",
            "number": pr.number.get(),
            "title": pr.title,
            "body": pr.body,
            "labels": pr.labels,
            "state": format!("{:?}", pr.state),
        })
    });
    let context = serde_json::json!({
        "queue": item.queue.as_str(),
        "kind": item.kind.as_str(),
        "artifact": artifact,
        "review_instruction": instruction,
    });
    Ok(serde_json::to_string_pretty(&context).unwrap_or_else(|_| context.to_string()))
}
