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
use std::collections::{BTreeMap, BTreeSet};
use temper_forge::{
    Forge, ForgeError, Issue, IssueState, ItemListDetails, ItemNumber, PullRequest,
    PullRequestState, RepositoryId,
};

/// Fresh same-repository lifecycle state collected by the current candidate
/// pass. Dependency links are deliberately absent: this index is only a target
/// state read-through optimization.
#[derive(Clone, Debug, Default)]
pub(crate) struct DependencyStateIndex {
    issues: BTreeMap<ItemNumber, IssueState>,
    pull_requests: BTreeMap<ItemNumber, PullRequestState>,
}

impl DependencyStateIndex {
    pub(crate) fn from_candidates(issues: &[Issue], pull_requests: &[PullRequest]) -> Self {
        Self {
            issues: issues
                .iter()
                .map(|issue| (issue.number, issue.state))
                .collect(),
            pull_requests: pull_requests
                .iter()
                .map(|pull_request| (pull_request.number, pull_request.state))
                .collect(),
        }
    }
}

pub async fn status_for_artifact<F: Forge + ?Sized>(
    forge: &F,
    repo_id: &RepositoryId,
    artifact: &ClassifiedArtifact,
) -> DependencyStatus {
    status_for_artifacts(forge, repo_id, std::iter::once(artifact)).await
}

pub async fn status_for_artifacts<'a, F: Forge + ?Sized>(
    forge: &F,
    repo_id: &RepositoryId,
    artifacts: impl IntoIterator<Item = &'a ClassifiedArtifact>,
) -> DependencyStatus {
    status_for_artifacts_with_index(forge, repo_id, artifacts, None).await
}

pub(crate) async fn status_for_artifacts_with_index<'a, F: Forge + ?Sized>(
    forge: &F,
    repo_id: &RepositoryId,
    artifacts: impl IntoIterator<Item = &'a ClassifiedArtifact>,
    index: Option<&DependencyStateIndex>,
) -> DependencyStatus {
    let mut status = DependencyStatus::new();
    for target in dependency_targets(artifacts) {
        match target_landed(forge, repo_id, &target, index).await {
            Ok(true) => status.mark_landed(target),
            Ok(false) => {}
            Err(error) => status.mark_read_failure(target, error.to_string()),
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
    index: Option<&DependencyStateIndex>,
) -> Result<bool, ForgeError> {
    let target_repo = target.resolved_repository(repo_id);
    let same_repo_index = target.is_in_repository(repo_id).then_some(index).flatten();

    // Forge providers use a single item-number namespace, but the reference
    // backends keep issue and pull-request counters independently. Preserve
    // issue-first collision semantics: a listed issue wins immediately;
    // otherwise probe the issue summary API before considering any listed PR.
    if let Some(state) = same_repo_index.and_then(|index| index.issues.get(&target.number)) {
        return Ok(*state == IssueState::Closed);
    }
    if let Some(issue) = forge
        .get_issue_by_number_with_details(&target_repo, target.number, ItemListDetails::summary())
        .await?
    {
        return Ok(issue.state == IssueState::Closed);
    }

    if let Some(state) = same_repo_index.and_then(|index| index.pull_requests.get(&target.number)) {
        return Ok(*state == PullRequestState::Merged);
    }
    Ok(forge
        .get_pull_request_by_number_with_details(
            &target_repo,
            target.number,
            ItemListDetails::summary(),
        )
        .await?
        .is_some_and(|pull_request| pull_request.state == PullRequestState::Merged))
}
