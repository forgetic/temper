use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Mutex;
use temper_forge::{Forge, ItemNumber};
use temper_runner::{Agent, AgentError, RoleTools, WorkItem};
use temper_workflow::ArtifactSource;

use super::support::run_or_ignore_stale;

#[derive(Clone, Debug, Default)]
pub struct FakeReviewer;

#[derive(Debug, Default)]
pub struct RequestChangesThenApproveReviewer {
    visits: Mutex<BTreeMap<ItemNumber, u64>>,
}

impl RequestChangesThenApproveReviewer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The transition this reviewer should attempt next for `number`, based on
    /// how many of its reviews have **landed** so far: `request_changes` first,
    /// then `approve_review`.
    fn pending_transition(&self, number: ItemNumber) -> &'static str {
        let visits = self
            .visits
            .lock()
            .expect("reviewer visit mutex is poisoned");
        if visits.get(&number).copied().unwrap_or(0) == 0 {
            "request_changes"
        } else {
            "approve_review"
        }
    }

    /// Records that a review **succeeded** for `number`, advancing the counter.
    ///
    /// Crucially this is called only after the transition actually lands, not on
    /// every visit: a stale/skipped first attempt must not "consume" the
    /// request-changes step and let the next visit jump straight to approval. On
    /// a real backend the first scan can race ahead of the PR being review-ready,
    /// so advancing only on success keeps "request changes, then approve" intact.
    fn record_success(&self, number: ItemNumber) {
        let mut visits = self
            .visits
            .lock()
            .expect("reviewer visit mutex is poisoned");
        let visit = visits.entry(number).or_insert(0);
        *visit = visit.saturating_add(1);
    }
}

#[derive(Clone, Debug, Default)]
pub struct FakeOwner;

#[derive(Clone, Debug, Default)]
pub struct FakeHuman;

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for FakeReviewer {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        if item.queue.as_str() == "pr_needs_review" && item.kind.as_str() == "implementation_pr" {
            return run_or_ignore_stale(tools, item.target, "approve_review").await;
        }
        Ok(false)
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for RequestChangesThenApproveReviewer {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        if item.queue.as_str() != "pr_needs_review" || item.kind.as_str() != "implementation_pr" {
            return Ok(false);
        }
        let ArtifactSource::PullRequest { number } = item.target else {
            return Ok(false);
        };
        let transition = self.pending_transition(number);
        let changed = run_or_ignore_stale(tools, item.target, transition).await?;
        // Advance only when the review actually landed, so a stale first attempt
        // does not skip the request-changes step.
        if changed && transition == "request_changes" {
            self.record_success(number);
        }
        Ok(changed)
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for FakeOwner {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        if item.kind.as_str() == "implementation_pr" && item.queue.as_str() == "owner_alignment" {
            return run_or_ignore_stale(tools, item.target, "review_alignment").await;
        }
        if item.queue.as_str() == "needs_owner" && item.kind.as_str() == "design" {
            return run_or_ignore_stale(tools, item.target, "request_human_input").await;
        }
        Ok(false)
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for FakeHuman {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        if item.queue.as_str() == "needs_human" && item.kind.as_str() == "design" {
            return run_or_ignore_stale(tools, item.target, "clear_human_flag").await;
        }
        Ok(false)
    }
}
