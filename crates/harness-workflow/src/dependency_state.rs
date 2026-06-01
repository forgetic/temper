//! Runtime derivation of dependency-gate signals from Forge state.
//!
//! The planner remains pure: it receives a [`DependencyStatus`](crate::DependencyStatus)
//! and only tests set membership. Runtime layers use this module to reduce the
//! classified `dependency` relations plus fresh Forge artifact state into that
//! signal. An issue dependency is landed when the target issue is closed; a pull
//! request dependency is landed when the target pull request is merged.

use crate::artifact::ArtifactRef;
use crate::classify::ClassifiedArtifact;
use crate::plan::DependencyStatus;
use crate::relation::RelationKind;
use harness_forge::{
    Forge, ForgeError, Issue, IssueState, PullRequest, PullRequestState, RepositoryId,
};
use std::collections::BTreeSet;

pub(crate) async fn status_for_artifact<F: Forge + ?Sized>(
    forge: &F,
    repo_id: &RepositoryId,
    artifact: &ClassifiedArtifact,
) -> Result<DependencyStatus, ForgeError> {
    let mut status = DependencyStatus::new();
    for target in dependency_targets(std::iter::once(artifact)) {
        if target_landed(forge, repo_id, &target).await? {
            status.mark_landed(target);
        }
    }
    Ok(status)
}

pub(crate) fn status_from_records<'a>(
    artifacts: impl IntoIterator<Item = &'a ClassifiedArtifact>,
    issues: &[Issue],
    pull_requests: &[PullRequest],
) -> DependencyStatus {
    let mut status = DependencyStatus::new();
    for target in dependency_targets(artifacts) {
        if target_landed_in_records(&target, issues, pull_requests) {
            status.mark_landed(target);
        }
    }
    status
}

fn dependency_targets<'a>(
    artifacts: impl IntoIterator<Item = &'a ClassifiedArtifact>,
) -> BTreeSet<ArtifactRef> {
    artifacts
        .into_iter()
        .flat_map(|artifact| artifact.relations.iter())
        .filter(|relation| relation.kind == RelationKind::Dependency)
        .map(|relation| relation.target.clone())
        .collect()
}

async fn target_landed<F: Forge + ?Sized>(
    forge: &F,
    repo_id: &RepositoryId,
    target: &ArtifactRef,
) -> Result<bool, ForgeError> {
    if !target.is_in_repository(repo_id) {
        // Phase 1 only introduces repo-qualified references. Phase 4 will
        // resolve cross-repo targets against their own repositories here.
        return Ok(false);
    }
    if forge
        .get_issue_by_number(repo_id, target.number)
        .await?
        .is_some_and(|issue| issue.state == IssueState::Closed)
    {
        return Ok(true);
    }
    Ok(forge
        .get_pull_request_by_number(repo_id, target.number)
        .await?
        .is_some_and(|pull_request| pull_request.state == PullRequestState::Merged))
}

fn target_landed_in_records(
    target: &ArtifactRef,
    issues: &[Issue],
    pull_requests: &[PullRequest],
) -> bool {
    issues.iter().any(|issue| {
        target.is_in_repository(&issue.repo_id)
            && issue.number == target.number
            && issue.state == IssueState::Closed
    }) || pull_requests.iter().any(|pull_request| {
        target.is_in_repository(&pull_request.repo_id)
            && pull_request.number == target.number
            && pull_request.state == PullRequestState::Merged
    })
}
