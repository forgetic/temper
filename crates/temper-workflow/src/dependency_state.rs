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
use std::collections::BTreeSet;
use temper_forge::{Forge, ForgeError, IssueState, PullRequestState, RepositoryId};

pub(crate) async fn status_for_artifact<F: Forge + ?Sized>(
    forge: &F,
    repo_id: &RepositoryId,
    artifact: &ClassifiedArtifact,
) -> DependencyStatus {
    status_for_artifacts(forge, repo_id, std::iter::once(artifact)).await
}

pub(crate) async fn status_for_artifacts<'a, F: Forge + ?Sized>(
    forge: &F,
    repo_id: &RepositoryId,
    artifacts: impl IntoIterator<Item = &'a ClassifiedArtifact>,
) -> DependencyStatus {
    let mut status = DependencyStatus::new();
    for target in dependency_targets(artifacts) {
        match target_landed(forge, repo_id, &target).await {
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
) -> Result<bool, ForgeError> {
    let target_repo = target.resolved_repository(repo_id);
    if forge
        .get_issue_by_number(&target_repo, target.number)
        .await?
        .is_some_and(|issue| issue.state == IssueState::Closed)
    {
        return Ok(true);
    }
    Ok(forge
        .get_pull_request_by_number(&target_repo, target.number)
        .await?
        .is_some_and(|pull_request| pull_request.state == PullRequestState::Merged))
}
