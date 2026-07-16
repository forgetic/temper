//! Atomic recovery of worker-pushed pull-request heads.

use temper_forge::{
    Forge, ForgeError, PullRequestState, RepositoryId, RequestReviewers, UpdatePullRequest, UserId,
};

use super::AssignmentConvergenceError;
use crate::classify::ArtifactSource;
use crate::ids::{ArtifactKindId, RoleId};
use crate::metadata::{DurableAssignment, parse_metadata_block, replace_metadata_block};
use crate::validated::{Effect, ValidatedTransition, ValidatedWorkflow};

/// Recovers a worker-pushed PR head before ordinary assignment rollback.
///
/// The complete assignment snapshot is checked on every retry and the declared
/// transition, repaired-head marker, assignment removal, and lease removal are
/// committed in one conditional update.
pub async fn recover_advanced_pull_request_assignment_from_durable<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    target: ArtifactSource,
    expected: &DurableAssignment,
    kind: ArtifactKindId,
    workflow: &ValidatedWorkflow,
) -> Result<bool, AssignmentConvergenceError> {
    let ArtifactSource::PullRequest { number } = target else {
        return Ok(false);
    };
    let (Some(role), Some(action)) = (expected.role.as_ref(), expected.action.as_deref()) else {
        return Ok(false);
    };
    let transition = workflow
        .transitions()
        .iter()
        .find(|transition| transition.id.as_str() == action)
        .ok_or_else(|| {
            AssignmentConvergenceError::InvalidContract(format!(
                "durable PR repair assignment names unknown action `{action}`"
            ))
        })?;
    validate_pr_transition(transition, role, &kind, action)?;
    let current_user = forge.current_user().await.ok().map(|user| user.id);

    for _ in 0..3 {
        let Some(pull_request) = forge.get_pull_request_by_number(repo, number).await? else {
            return Ok(false);
        };
        if pull_request.state != PullRequestState::Open {
            return Ok(false);
        }
        let Some(current_head) = pull_request
            .head_sha
            .as_deref()
            .map(str::trim)
            .filter(|head| !head.is_empty())
        else {
            return Ok(false);
        };
        let mut metadata = parse_metadata_block(&pull_request.body)
            .map_err(|error| AssignmentConvergenceError::InvalidContract(error.to_string()))?
            .unwrap_or_default();
        if metadata.assignment.as_ref() != Some(expected) {
            return Ok(false);
        }
        let Some(assignment_head) = expected
            .assignment_pr_head
            .as_deref()
            .map(str::trim)
            .filter(|head| !head.is_empty())
        else {
            return Ok(false);
        };
        if assignment_head == current_head {
            return Ok(false);
        }

        let mut add_labels = Vec::new();
        let mut remove_labels = Vec::new();
        let mut add_assignees = Vec::new();
        let mut remove_assignees = Vec::new();
        let mut reviewer_roles = Vec::new();
        for effect in &transition.effects {
            if !effect.supports_pull_request_repair_publication() {
                return Err(AssignmentConvergenceError::InvalidContract(format!(
                    "durable PR repair action `{action}` cannot be recovered atomically"
                )));
            }
            match effect {
                Effect::AddLabel(label) => push_unique_string(&mut add_labels, label.as_str()),
                Effect::RemoveLabel(label) | Effect::RemoveLabelIfPresent(label) => {
                    if label.as_str() != "landing" {
                        push_unique_string(&mut remove_labels, label.as_str());
                    }
                }
                Effect::SetAssignee(effect_role) => push_unique_user(
                    &mut add_assignees,
                    assignment_role_user(effect_role, role, current_user.as_ref()),
                ),
                Effect::RemoveAssignee(effect_role) => push_unique_user(
                    &mut remove_assignees,
                    assignment_role_user(effect_role, role, current_user.as_ref()),
                ),
                Effect::RequestReviewers { roles } => {
                    for reviewer in roles {
                        if !reviewer_roles.contains(reviewer) {
                            reviewer_roles.push(reviewer.clone());
                        }
                    }
                }
                _ => unreachable!("unsupported repair effect rejected above"),
            }
        }

        metadata.assignment = None;
        metadata.lease = None;
        metadata.repaired_head = Some(current_head.to_string());
        let body = replace_metadata_block(&pull_request.body, &metadata)
            .map_err(|error| AssignmentConvergenceError::InvalidContract(error.to_string()))?;
        match forge
            .update_pull_request(
                &pull_request.id,
                UpdatePullRequest {
                    body: Some(body),
                    add_labels,
                    remove_labels,
                    add_assignees,
                    remove_assignees,
                    expected_version: Some(pull_request.version),
                    ..UpdatePullRequest::default()
                },
            )
            .await
        {
            Ok(committed) => {
                let reviewers = reviewer_roles
                    .into_iter()
                    .map(|reviewer| UserId::new(reviewer.as_str()))
                    .filter(|reviewer| !committed.requested_reviewers.contains(reviewer))
                    .collect::<Vec<_>>();
                if !reviewers.is_empty() {
                    if let Err(error) = forge
                        .request_pull_request_reviewers(
                            &committed.id,
                            RequestReviewers { reviewers },
                        )
                        .await
                    {
                        tracing::warn!(
                            pull_request = %committed.number,
                            %error,
                            "recovered PR repair transition but could not request reviewers"
                        );
                    }
                }
                return Ok(true);
            }
            Err(ForgeError::Conflict(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(ForgeError::Conflict(format!(
        "pull request #{number} changed during repair recovery"
    ))
    .into())
}

fn validate_pr_transition(
    transition: &ValidatedTransition,
    role: &RoleId,
    kind: &ArtifactKindId,
    action: &str,
) -> Result<(), AssignmentConvergenceError> {
    if &transition.artifact != kind || !transition.roles.contains(role) {
        return Err(AssignmentConvergenceError::InvalidContract(format!(
            "durable PR repair assignment is not authorized for action `{action}`"
        )));
    }
    Ok(())
}

fn assignment_role_user(
    role: &RoleId,
    assignment_role: &RoleId,
    current_user: Option<&UserId>,
) -> UserId {
    if role == assignment_role {
        current_user
            .cloned()
            .unwrap_or_else(|| UserId::new(role.as_str()))
    } else {
        UserId::new(role.as_str())
    }
}

fn push_unique_string(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn push_unique_user(values: &mut Vec<UserId>, value: UserId) {
    if !values.contains(&value) {
        values.push(value);
    }
}
