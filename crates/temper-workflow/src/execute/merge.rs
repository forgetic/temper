//! Pull-request merge application and merge-rejection classification.
//!
//! Kept separate from `apply.rs` so the side-effect applier stays within the
//! source-size budget while merge-specific retry and conflict-routing rules stay
//! easy to audit.

use super::{ExecutionError, Executor, Loaded};
use crate::classify::ArtifactSource;
use crate::plan::WorkflowEffect;
use temper_forge::{
    Forge, ForgeError, MergeMethod, MergePullRequest, PullRequestState, RepositoryId,
};

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Merges the target pull request at most once.
    ///
    /// Runs before the label commit point so that the post-merge labels (which
    /// the transition declares as ordinary `add_label` effects) double as the
    /// "already done" marker: once they land, a retry's planner sees them
    /// present and refuses to re-run. A pull request that is already merged
    /// (observed in the freshly loaded state) is skipped, so a crash that lands
    /// the merge but loses the response never merges twice on retry.
    ///
    /// When the backend reports a merge conflict/rejection, the executor
    /// re-reads the pull request before deciding what kind of workflow outcome
    /// occurred. Already-merged means the merge landed despite the response and
    /// post-merge projection may continue; missing/closed targets are stale;
    /// still-open unmerged targets become a typed routable merge conflict.
    pub(super) async fn apply_merge(
        &self,
        repo_id: &RepositoryId,
        loaded: &Loaded,
        merge: bool,
    ) -> Result<(), ExecutionError> {
        if !merge {
            return Ok(());
        }
        let Loaded::PullRequest { id, merged, .. } = loaded else {
            return Err(ExecutionError::UnsupportedEffect {
                effect: WorkflowEffect::MergePullRequest,
            });
        };
        if *merged {
            return Ok(());
        }
        let target = loaded.classified().source;
        let input = MergePullRequest {
            method: MergeMethod::MergeCommit,
            commit_title: None,
            commit_body: None,
        };
        match self.forge.merge_pull_request(id, input).await {
            Ok(_) => Ok(()),
            Err(ForgeError::Conflict(message)) => {
                self.resolve_merge_rejection(repo_id, target, message).await
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn resolve_merge_rejection(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        message: String,
    ) -> Result<(), ExecutionError> {
        let ArtifactSource::PullRequest { number } = target else {
            return Err(ExecutionError::UnsupportedEffect {
                effect: WorkflowEffect::MergePullRequest,
            });
        };
        let pull_request = self
            .forge
            .get_pull_request_by_number(repo_id, number)
            .await?
            .ok_or(ExecutionError::TargetMissing { target })?;
        match pull_request.state {
            PullRequestState::Merged => Ok(()),
            PullRequestState::Open => Err(ExecutionError::MergeConflict {
                target,
                message: merge_rejection_summary(&message),
            }),
            PullRequestState::Closed => Err(ExecutionError::TargetStale {
                target,
                message: format!(
                    "merge was rejected and the pull request is now closed: {}",
                    merge_rejection_summary(&message)
                ),
            }),
        }
    }
}

fn merge_rejection_summary(message: &str) -> String {
    let collapsed = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "provider reported a merge rejection".to_string();
    }
    let mut chars = collapsed.chars();
    let summary: String = chars.by_ref().take(240).collect();
    if chars.next().is_some() {
        format!("{summary}…")
    } else {
        summary
    }
}
