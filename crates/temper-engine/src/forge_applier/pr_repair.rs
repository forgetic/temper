// SPDX-License-Identifier: MPL-2.0

//! Monotonic publication and restart-safe handoff for in-place PR repairs.

use temper_forge::{
    Forge, ForgeError, PullRequest, PullRequestState, RequestReviewers, UpdatePullRequest, UserId,
};
use temper_protocol_worker::{JobContext, JobResult};
use temper_runner::implementation_pr_body_from_report_or_summary;
use temper_workflow::{
    ArtifactKindId, Effect, WorkflowMetadata, parse_metadata_block, replace_metadata_block,
};

use crate::InFlightJob;
use crate::applier::ApplyOutcome;
use crate::forge_applier::ForgeApplier;

impl<F: Forge + ?Sized> ForgeApplier<F> {
    /// Publishes an in-place PR repair as a monotonic, conditional workflow
    /// transition. The result is applicable only while the durable assignment
    /// still names this job and the PR still points at the worker-reported SHA.
    pub(super) async fn publish_pull_request_repair(
        &self,
        job: &InFlightJob,
        result: &JobResult,
    ) -> ApplyOutcome {
        let context = match serde_json::from_value::<JobContext>(job.job_payload.clone()) {
            Ok(context) => context,
            Err(error) => {
                return ApplyOutcome::Rejected {
                    class: temper_protocol_worker::FailureClass::Protocol,
                    reason: format!("invalid in-flight JobContext: {error}"),
                };
            }
        };
        if context.checkout_capability.as_deref() != Some("pull_request_writable") {
            return ApplyOutcome::Rejected {
                class: temper_protocol_worker::FailureClass::Protocol,
                reason: "pull-request success did not use pull_request_writable checkout"
                    .to_string(),
            };
        }
        let Some(freshness) = context.pull_request_freshness.as_ref() else {
            return ApplyOutcome::Rejected {
                class: temper_protocol_worker::FailureClass::Protocol,
                reason: "pull-request repair result has no assignment-head freshness guard"
                    .to_string(),
            };
        };
        let Some(action) = context.action.as_deref() else {
            return ApplyOutcome::Rejected {
                class: temper_protocol_worker::FailureClass::Protocol,
                reason: "pull-request repair result has no declared action".to_string(),
            };
        };
        if freshness.action != action {
            return ApplyOutcome::Rejected {
                class: temper_protocol_worker::FailureClass::Protocol,
                reason: "pull-request repair action disagrees with its freshness guard".to_string(),
            };
        }
        let Some(outcome) = result.repos.iter().find(|outcome| outcome.repo == job.repo) else {
            return ApplyOutcome::Rejected {
                class: temper_protocol_worker::FailureClass::Protocol,
                reason: "pull-request repair result has no outcome for its artifact repository"
                    .to_string(),
            };
        };
        let reported_head = outcome.branch.head_sha.trim();
        if reported_head.is_empty()
            || freshness.head_sha.as_deref().map(str::trim) == Some(reported_head)
        {
            return ApplyOutcome::Rejected {
                class: temper_protocol_worker::FailureClass::Protocol,
                reason: "pull-request repair did not report a new self-pushed head".to_string(),
            };
        }
        let transition = match repair_transition(
            self.workflow.as_ref(),
            action,
            &job.role,
            &context.artifact_kind,
        ) {
            Ok(transition) => transition,
            Err(reason) => {
                return ApplyOutcome::Rejected {
                    class: temper_protocol_worker::FailureClass::Protocol,
                    reason,
                };
            }
        };
        let current_role_user = self.forge.current_user().await.ok().map(|user| user.id);

        for _ in 0..3 {
            let Some((_, pull_request)) = self.resolve_pull_request(job).await else {
                return ApplyOutcome::Stale;
            };
            if pull_request.state != PullRequestState::Open
                || pull_request.id.as_str() != freshness.pull_request_id
                || pull_request.head_sha.as_deref().map(str::trim) != Some(reported_head)
            {
                return ApplyOutcome::Stale;
            }
            let mut metadata = match parse_metadata_block(&pull_request.body) {
                Ok(metadata) => metadata.unwrap_or_default(),
                Err(error) => {
                    return ApplyOutcome::Rejected {
                        class: temper_protocol_worker::FailureClass::Protocol,
                        reason: format!("invalid pull-request workflow metadata: {error}"),
                    };
                }
            };
            let Some(assignment) = metadata.assignment.as_ref() else {
                return ApplyOutcome::Stale;
            };
            if !repair_assignment_matches(
                assignment,
                job,
                result,
                &context,
                freshness.head_sha.as_deref(),
            ) {
                return ApplyOutcome::Stale;
            }
            if metadata.repaired_head.as_deref() == Some(reported_head) {
                return ApplyOutcome::Applied;
            }

            let mutation = transition.mutation(job, current_role_user.as_ref());
            metadata.repaired_head = Some(reported_head.to_string());
            let body = match replace_metadata_block(&pull_request.body, &metadata) {
                Ok(body) => body,
                Err(error) => {
                    return ApplyOutcome::Rejected {
                        class: temper_protocol_worker::FailureClass::Protocol,
                        reason: format!("could not persist repaired-head metadata: {error}"),
                    };
                }
            };
            match self
                .forge
                .update_pull_request(
                    &pull_request.id,
                    UpdatePullRequest {
                        body: Some(body),
                        add_labels: mutation.add_labels,
                        remove_labels: mutation.remove_labels,
                        add_assignees: mutation.add_assignees,
                        remove_assignees: mutation.remove_assignees,
                        expected_version: Some(pull_request.version),
                        ..UpdatePullRequest::default()
                    },
                )
                .await
            {
                Ok(committed) => {
                    self.apply_repair_reviewers(job, &committed, &transition)
                        .await;
                    // Handoff prose is deliberately secondary. Its optimistic
                    // update may fail without undoing the committed transition.
                    self.update_pull_request_repair_handoff(job, result).await;
                    tracing::debug!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        repo = %job.repo,
                        artifact_item = %job.artifact.item,
                        repaired_head = reported_head,
                        action,
                        "forge applier published pull-request repair transition; awaiting fresh CI"
                    );
                    return ApplyOutcome::Applied;
                }
                Err(ForgeError::Conflict(_)) => continue,
                Err(error) => {
                    return ApplyOutcome::Retryable {
                        reason: format!("could not publish pull-request repair: {error}"),
                    };
                }
            }
        }

        ApplyOutcome::Retryable {
            reason: "pull-request repair publication remained contended".to_string(),
        }
    }

    async fn apply_repair_reviewers(
        &self,
        job: &InFlightJob,
        pull_request: &PullRequest,
        transition: &RepairTransition,
    ) {
        let reviewers = transition
            .reviewer_roles
            .iter()
            .map(|role| UserId::new(role.as_str()))
            .filter(|reviewer| !pull_request.requested_reviewers.contains(reviewer))
            .collect::<Vec<_>>();
        if reviewers.is_empty() {
            return;
        }
        if let Err(error) = self
            .forge
            .request_pull_request_reviewers(&pull_request.id, RequestReviewers { reviewers })
            .await
        {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                pull_request = %pull_request.number,
                %error,
                "forge applier committed PR repair but could not request reviewers"
            );
        }
    }

    async fn update_pull_request_repair_handoff(&self, job: &InFlightJob, result: &JobResult) {
        let Some((_, pull_request)) = self.resolve_pull_request(job).await else {
            return;
        };
        let desired_title = result
            .title
            .as_deref()
            .and_then(non_blank)
            .map(str::to_string)
            .unwrap_or_else(|| pull_request.title.clone());
        let metadata = parse_metadata_block(&pull_request.body)
            .ok()
            .flatten()
            .unwrap_or_else(default_implementation_pr_metadata);
        let fallback_intro = format!(
            "Workspace-produced update for pull request #{}.",
            pull_request.number
        );
        let desired_body = implementation_pr_body_from_report_or_summary(
            result.body.as_deref(),
            &fallback_intro,
            result.summary.as_deref().unwrap_or_default(),
            &metadata,
        );
        let _ = self
            .update_implementation_pr_handoff(
                job,
                pull_request,
                &desired_title,
                &desired_body,
                "pull request repair",
            )
            .await;
    }
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|candidate| candidate == value) {
        values.push(value.to_string());
    }
}

#[derive(Default)]
struct RepairMutation {
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
    add_assignees: Vec<UserId>,
    remove_assignees: Vec<UserId>,
}

struct RepairTransition {
    effects: Vec<Effect>,
    reviewer_roles: Vec<temper_workflow::RoleId>,
}

impl RepairTransition {
    fn mutation(&self, job: &InFlightJob, current_role_user: Option<&UserId>) -> RepairMutation {
        let mut mutation = RepairMutation::default();
        for effect in &self.effects {
            match effect {
                Effect::AddLabel(label) => {
                    push_unique(&mut mutation.add_labels, label.as_str());
                }
                Effect::RemoveLabel(label) | Effect::RemoveLabelIfPresent(label) => {
                    push_unique(&mut mutation.remove_labels, label.as_str());
                }
                Effect::SetAssignee(role) => {
                    let user = repair_role_user(role, job, current_role_user);
                    if !mutation.add_assignees.contains(&user) {
                        mutation.add_assignees.push(user);
                    }
                }
                Effect::RemoveAssignee(role) => {
                    let user = repair_role_user(role, job, current_role_user);
                    if !mutation.remove_assignees.contains(&user) {
                        mutation.remove_assignees.push(user);
                    }
                }
                Effect::RequestReviewers { .. } => {}
                _ => unreachable!("unsupported repair effect was rejected during planning"),
            }
        }
        mutation
    }
}

fn repair_transition(
    workflow: &temper_workflow::ValidatedWorkflow,
    action: &str,
    role: &str,
    artifact_kind: &str,
) -> Result<RepairTransition, String> {
    let transition = workflow
        .transitions()
        .iter()
        .find(|transition| transition.id.as_str() == action)
        .ok_or_else(|| format!("pull-request repair action `{action}` is not declared"))?;
    if transition.artifact.as_str() != artifact_kind {
        return Err(format!(
            "pull-request repair action `{action}` operates on `{}`, not `{artifact_kind}`",
            transition.artifact
        ));
    }
    if !transition
        .roles
        .iter()
        .any(|candidate| candidate.as_str() == role)
    {
        return Err(format!(
            "role `{role}` is not authorized for pull-request repair action `{action}`"
        ));
    }
    let mut reviewer_roles = Vec::new();
    for effect in &transition.effects {
        if !effect.supports_pull_request_repair_publication() {
            return Err(format!(
                "pull-request repair action `{action}` contains an effect that cannot be published atomically"
            ));
        }
        if let Effect::RequestReviewers { roles } = effect {
            for role in roles {
                if !reviewer_roles.contains(role) {
                    reviewer_roles.push(role.clone());
                }
            }
        }
    }
    Ok(RepairTransition {
        effects: transition.effects.clone(),
        reviewer_roles,
    })
}

fn repair_assignment_matches(
    assignment: &temper_workflow::DurableAssignment,
    job: &InFlightJob,
    result: &JobResult,
    context: &JobContext,
    assignment_head: Option<&str>,
) -> bool {
    assignment.job_id.as_deref() == Some(job.job_id.as_str())
        && assignment.role.as_ref().map(|role| role.as_str()) == Some(job.role.as_str())
        && assignment.queue.as_deref() == Some(context.queue.as_str())
        && assignment.action.as_deref() == context.action.as_deref()
        && assignment.worker_id.as_deref() == Some(result.worker_id.as_str())
        && assignment.assignment_pr_head.as_deref() == assignment_head
        && assignment.coordination_key.as_deref()
            == context
                .workspace
                .as_ref()
                .map(|workspace| workspace.coordination_key.as_str())
}

fn repair_role_user(
    role: &temper_workflow::RoleId,
    job: &InFlightJob,
    current_role_user: Option<&UserId>,
) -> UserId {
    if role.as_str() == job.role {
        current_role_user
            .cloned()
            .unwrap_or_else(|| UserId::new(role.as_str()))
    } else {
        UserId::new(role.as_str())
    }
}

fn non_blank(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn default_implementation_pr_metadata() -> WorkflowMetadata {
    WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        ..WorkflowMetadata::default()
    }
}
