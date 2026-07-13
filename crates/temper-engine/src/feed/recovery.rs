// SPDX-License-Identifier: MPL-2.0

//! Broad-only recovery of worker-pushed pull-request assignments.

use temper_forge::{Forge, PullRequestQuery, PullRequestState, RepositoryId};
use temper_runner::{ScanError, WorkItem};
use temper_workflow::{ArtifactSource, RoleId, ValidatedWorkflow, parse_metadata_block};

use super::recover_advanced_pull_request_assignment;

pub(super) async fn recover_advanced_pull_request_assignments<F: Forge + ?Sized>(
    daemon: &crate::Daemon,
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    role: &RoleId,
) -> Result<(), ScanError> {
    recover_advanced_pull_request_assignments_for_roles(
        daemon,
        forge,
        repo,
        workflow,
        std::slice::from_ref(role),
    )
    .await
}

pub(super) async fn recover_advanced_pull_request_assignments_for_roles<F: Forge + ?Sized>(
    daemon: &crate::Daemon,
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    roles: &[RoleId],
) -> Result<(), ScanError> {
    let pull_requests = forge
        .list_pull_requests(
            repo,
            PullRequestQuery {
                state: Some(PullRequestState::Open),
                ..PullRequestQuery::default()
            },
        )
        .await?;
    for pull_request in pull_requests {
        let Some(current_head) = pull_request.head_sha.as_deref() else {
            continue;
        };
        let metadata = match parse_metadata_block(&pull_request.body) {
            Ok(metadata) => metadata.unwrap_or_default(),
            Err(error) => {
                tracing::warn!(
                    pull_request = %pull_request.number,
                    %error,
                    "could not inspect PR assignment metadata during repair recovery"
                );
                continue;
            }
        };
        if metadata.staged {
            continue;
        }
        let Some(assignment) = metadata.assignment.as_ref() else {
            continue;
        };
        let (Some(role), Some(queue), Some(action)) = (
            assignment.role.as_ref().filter(|role| roles.contains(role)),
            assignment.queue.as_deref(),
            assignment.action.as_deref(),
        ) else {
            continue;
        };
        if assignment.assignment_pr_head.as_deref() == Some(current_head) {
            continue;
        }
        let Some(coordination_key) = assignment.coordination_key.as_deref() else {
            continue;
        };
        if daemon
            .workstream_active_by_correlation_key(coordination_key)
            .await
        {
            // The daemon that owns this assignment is still tracking the
            // worker. Only an empty (restarted) dispatch core recovers a push
            // for which no result can still be delivered locally.
            continue;
        }
        let Some(transition) = workflow
            .transitions()
            .iter()
            .find(|transition| transition.id.as_str() == action)
        else {
            continue;
        };
        let item = WorkItem {
            queue: temper_workflow::QueueId::new(queue),
            role: role.clone(),
            target: ArtifactSource::PullRequest {
                number: pull_request.number,
            },
            kind: transition.artifact.clone(),
        };
        recover_advanced_pull_request_assignment(forge, repo, &item, workflow).await?;
    }
    Ok(())
}
