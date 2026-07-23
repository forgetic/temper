// SPDX-License-Identifier: MPL-2.0

//! Validation for existing pull requests considered by coordinated success
//! publication.

use temper_forge::{
    BranchRef, Forge, PullRequest, PullRequestQuery, PullRequestState, RepositoryId,
};
use temper_protocol_worker::{FailureClass, RepoOutcome};
use temper_workflow::{ExecutionError, Executor, validate_pull_request_topology};

use crate::InFlightJob;
use crate::applier::ApplyOutcome;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::coordinated::{CoordinatedSet, pr_target_branch};

impl<F: Forge + ?Sized> ForgeApplier<F> {
    /// Validates every existing branch- or correlation-reuse candidate before
    /// any member of a coordinated set advances its handoff. This keeps a
    /// divergent candidate in a later repository from partially publishing the
    /// set before the source issue is quarantined.
    pub(super) async fn validate_existing_coordinated_pr_topologies(
        &self,
        set: &CoordinatedSet<'_>,
        outcomes: &[RepoOutcome],
    ) -> Result<(), ApplyOutcome> {
        for outcome in outcomes {
            let source_branch = outcome.branch.name.trim();
            if source_branch.is_empty() {
                continue;
            }
            let Some(target_repository) = self.resolve_repo_path(set.job, &outcome.repo).await
            else {
                continue;
            };
            let source = BranchRef {
                repository_id: target_repository.id.clone(),
                branch: source_branch.to_string(),
            };
            let target = BranchRef {
                repository_id: target_repository.id.clone(),
                branch: pr_target_branch(set, &outcome.repo, &target_repository),
            };

            if let Some(candidate) = self
                .existing_open_pr_for_branch(
                    set.job,
                    &target_repository.id,
                    source_branch,
                    set.lookup_labels,
                )
                .await
                .map_err(pull_request_reuse_error)?
            {
                validate_pull_request_topology(&candidate, &source, &target)
                    .map_err(pull_request_reuse_error)?;
            }

            if let Some(candidate) = Executor::new(self.workflow.as_ref(), self.forge.as_ref())
                .find_pull_request_by_correlation(
                    &target_repository.id,
                    set.coordination_key,
                    set.lookup_labels,
                )
                .await
                .map_err(pull_request_reuse_error)?
            {
                validate_pull_request_topology(&candidate, &source, &target)
                    .map_err(pull_request_reuse_error)?;
            }
        }
        Ok(())
    }

    pub(super) async fn existing_open_pr_for_branch(
        &self,
        job: &InFlightJob,
        repo_id: &RepositoryId,
        source_branch: &str,
        labels: &[String],
    ) -> Result<Option<PullRequest>, ExecutionError> {
        let source_branch = source_branch.trim();
        if source_branch.is_empty() {
            return Ok(None);
        }
        let query = PullRequestQuery {
            state: Some(PullRequestState::Open),
            labels: labels.to_vec(),
            ..PullRequestQuery::default()
        };
        self.forge
            .list_pull_requests(repo_id, query)
            .await
            .map(|pull_requests| {
                pull_requests.into_iter().find(|pull_request| {
                    pull_request.source.repository_id == *repo_id
                        && pull_request.source.branch == source_branch
                })
            })
            .map_err(|error| {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    target_repo = %repo_id,
                    source_branch,
                    %error,
                    "forge applier could not look up existing PR by source branch"
                );
                error.into()
            })
    }
}

pub(super) fn pull_request_reuse_error(error: ExecutionError) -> ApplyOutcome {
    match error {
        ExecutionError::Backend { .. } => ApplyOutcome::Retryable {
            reason: error.to_string(),
        },
        _ => ApplyOutcome::Rejected {
            class: FailureClass::Protocol,
            reason: error.to_string(),
        },
    }
}
