// SPDX-License-Identifier: MPL-2.0

//! Source-artifact claim and completion signal application for workspace jobs.
//!
//! A workspace-backed issue action such as `open_pr` has two observable phases:
//! the worker starts advancing the source issue, then later reports the pushed
//! branch from which the daemon opens/ensures the implementation PR. The latter
//! path is intentionally custom because coordinated PR creation needs runtime
//! branch and metadata inputs, but the source issue still owns useful workflow
//! effects such as `-ready +in-progress` and `set_assignee`. This module applies
//! those source effects idempotently without invoking the PR-creation effect.

use temper_forge::{Forge, ForgeError, UpdateIssue, UpdatePullRequest, UserId};
use temper_protocol_worker::JobContext;
use temper_workflow::{ArtifactKindId, Effect, LabelId, RoleId, ValidatedWorkflow};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;

impl<F: Forge + ?Sized> ForgeApplier<F> {
    /// Applies the source-artifact claim effects for the assigned workflow
    /// action.
    ///
    /// Assignee effects are always safe claim signals. Label effects are applied
    /// only for actions that also declare `create_pull_request`, because those
    /// actions use labels such as `ready`/`in-progress` to represent the source
    /// issue's implementation lifecycle while the PR is being prepared. The
    /// actual `create_pull_request` effect is skipped here; the success path
    /// materializes PRs from the worker's repo outcomes.
    pub(super) async fn apply_source_action_claim(&self, job: &InFlightJob) {
        let Some(effects) = self.action_effects(job) else {
            tracing::debug!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                role = %job.role,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                "forge applier found no source action effects to claim"
            );
            return;
        };
        let include_label_effects = effects
            .iter()
            .any(|effect| matches!(effect, Effect::CreatePullRequest { .. }));
        let current_role_user = self.current_role_user(job).await;
        let mutation = claim_mutation(job, &effects, include_label_effects, current_role_user);
        tracing::debug!(
            target: "temper_daemon",
            job_id = %job.job_id,
            repo = %job.repo,
            role = %job.role,
            artifact_kind = %job.artifact.kind,
            artifact_item = %job.artifact.item,
            include_label_effects,
            add_labels = mutation.add_labels.len(),
            remove_labels = mutation.remove_labels.len(),
            add_assignees = mutation.add_assignees.len(),
            remove_assignees = mutation.remove_assignees.len(),
            "forge applier computed source mutation"
        );
        self.apply_source_mutation(job, mutation, "claim source action")
            .await;
    }

    /// Reverses a source-action claim after a retryable worker failure, but
    /// only while the source artifact still appears to be in the exact claimed
    /// state for this action. This makes a claimed issue queue-visible again
    /// without disturbing an artifact that a peer, human, or later worker has
    /// already advanced to another state.
    pub(super) async fn release_source_action_claim_for_retry(&self, job: &InFlightJob) -> bool {
        if job.artifact.kind != "issue" {
            return false;
        }

        let context = match serde_json::from_value::<JobContext>(job.job_payload.clone()) {
            Ok(context) => context,
            Err(error) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    artifact_kind = %job.artifact.kind,
                    artifact_item = %job.artifact.item,
                    %error,
                    "forge applier could not parse JobContext for retry claim release"
                );
                return false;
            }
        };
        let Some(action) = context.action.as_deref() else {
            return false;
        };
        let Some(transition) = self
            .workflow
            .transitions()
            .iter()
            .find(|transition| transition.id.as_str() == action)
        else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                role = %job.role,
                action,
                "forge applier could not find action transition for retry claim release"
            );
            return false;
        };

        let effects = transition.effects.clone();
        let include_label_effects = effects
            .iter()
            .any(|effect| matches!(effect, Effect::CreatePullRequest { .. }));
        if !include_label_effects {
            return false;
        }

        let current_role_user = self.current_role_user(job).await;
        let mutation = self.retry_release_mutation(job, &effects, current_role_user);
        if mutation.is_empty() {
            return false;
        }

        let artifact_kind = ArtifactKindId::new(context.artifact_kind);
        self.apply_retry_release_mutation(job, &effects, &artifact_kind, mutation)
            .await
    }

    /// Clears working labels that the source action added now that the worker's
    /// successful result has been materialized as implementation PR(s).
    pub(super) async fn clear_source_action_working_labels(&self, job: &InFlightJob) {
        let Some(effects) = self.action_effects(job) else {
            return;
        };
        let mut mutation = SourceMutation::default();
        for effect in effects {
            if let Effect::AddLabel(label) = effect
                && self.is_working_label(&label)
            {
                push_unique(&mut mutation.remove_labels, label.as_str().to_string());
            }
        }
        self.apply_source_mutation(job, mutation, "complete source action")
            .await;
    }

    fn action_effects(&self, job: &InFlightJob) -> Option<Vec<Effect>> {
        let context = match serde_json::from_value::<JobContext>(job.job_payload.clone()) {
            Ok(context) => context,
            Err(error) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    artifact_kind = %job.artifact.kind,
                    artifact_item = %job.artifact.item,
                    %error,
                    "forge applier could not parse JobContext for source action signals"
                );
                return None;
            }
        };
        let action = context.action?;
        let transition = self
            .workflow
            .transitions()
            .iter()
            .find(|transition| transition.id.as_str() == action.as_str());
        match transition {
            Some(transition) => Some(transition.effects.clone()),
            None => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    role = %job.role,
                    action = %action,
                    "forge applier could not find action transition for source signals"
                );
                None
            }
        }
    }

    async fn current_role_user(&self, job: &InFlightJob) -> Option<UserId> {
        match self.forge.current_user().await {
            Ok(user) => Some(user.id),
            Err(error) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    role = %job.role,
                    %error,
                    "forge applier could not resolve current user for source action signals"
                );
                None
            }
        }
    }

    fn is_working_label(&self, label: &LabelId) -> bool {
        label.as_str() == "in-progress"
            || self.workflow.state_dimensions().iter().any(|dimension| {
                dimension.states.iter().any(|state| {
                    state.id.as_str() == "in_progress"
                        && state
                            .label
                            .as_ref()
                            .is_some_and(|candidate| candidate == label)
                })
            })
    }

    async fn apply_source_mutation(
        &self,
        job: &InFlightJob,
        mutation: SourceMutation,
        operation: &'static str,
    ) {
        if mutation.is_empty() {
            tracing::debug!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                role = %job.role,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                operation,
                "forge applier skipped empty source mutation"
            );
            return;
        }

        match job.artifact.kind.as_str() {
            "issue" => {
                let Some((_, issue)) = self.resolve_issue(job).await else {
                    return;
                };
                if let Err(error) = self
                    .forge
                    .update_issue(
                        &issue.id,
                        UpdateIssue {
                            add_labels: mutation.add_labels,
                            remove_labels: mutation.remove_labels,
                            add_assignees: mutation.add_assignees,
                            remove_assignees: mutation.remove_assignees,
                            ..UpdateIssue::default()
                        },
                    )
                    .await
                {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        repo = %job.repo,
                        issue = %issue.number,
                        %error,
                        "forge applier could not {operation} on source issue"
                    );
                }
            }
            "pull_request" => {
                let Some((_, pull_request)) = self.resolve_pull_request(job).await else {
                    return;
                };
                if let Err(error) = self
                    .forge
                    .update_pull_request(
                        &pull_request.id,
                        UpdatePullRequest {
                            add_labels: mutation.add_labels,
                            remove_labels: mutation.remove_labels,
                            add_assignees: mutation.add_assignees,
                            remove_assignees: mutation.remove_assignees,
                            ..UpdatePullRequest::default()
                        },
                    )
                    .await
                {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        repo = %job.repo,
                        pull_request = %pull_request.number,
                        %error,
                        "forge applier could not {operation} on source pull request"
                    );
                }
            }
            _ => {}
        }
    }
    async fn apply_retry_release_mutation(
        &self,
        job: &InFlightJob,
        effects: &[Effect],
        artifact_kind: &ArtifactKindId,
        mutation: SourceMutation,
    ) -> bool {
        for _ in 0..3 {
            let Some((_, issue)) = self.resolve_issue(job).await else {
                return false;
            };
            if !source_claim_labels_still_current(
                &issue.labels,
                effects,
                artifact_kind,
                self.workflow.as_ref(),
            ) {
                return false;
            }

            match self
                .forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        add_labels: mutation.add_labels.clone(),
                        remove_labels: mutation.remove_labels.clone(),
                        add_assignees: mutation.add_assignees.clone(),
                        remove_assignees: mutation.remove_assignees.clone(),
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(_) => return true,
                Err(ForgeError::Conflict(_)) => continue,
                Err(error) => {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        repo = %job.repo,
                        issue = %issue.number,
                        %error,
                        "forge applier could not release source claim for retry"
                    );
                    return false;
                }
            }
        }

        tracing::warn!(
            target: "temper_daemon",
            job_id = %job.job_id,
            repo = %job.repo,
            artifact_kind = %job.artifact.kind,
            artifact_item = %job.artifact.item,
            "forge applier gave up releasing source claim for retry after conflicts"
        );
        false
    }

    fn retry_release_mutation(
        &self,
        job: &InFlightJob,
        effects: &[Effect],
        current_role_user: Option<UserId>,
    ) -> SourceMutation {
        let mut mutation = SourceMutation::default();
        for effect in effects {
            match effect {
                Effect::AddLabel(label) if self.is_working_label(label) => {
                    push_unique(&mut mutation.remove_labels, label.as_str().to_string());
                }
                Effect::RemoveLabel(label) => {
                    push_unique(&mut mutation.add_labels, label.as_str().to_string());
                }
                Effect::SetAssignee(role) => {
                    let user = resolve_effect_role_user(role, job, current_role_user.as_ref());
                    push_unique(&mut mutation.remove_assignees, user);
                }
                Effect::RemoveAssignee(role) => {
                    let user = resolve_effect_role_user(role, job, current_role_user.as_ref());
                    push_unique(&mut mutation.add_assignees, user);
                }
                _ => {}
            }
        }
        mutation
    }
}

#[derive(Default, Clone)]
struct SourceMutation {
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
    add_assignees: Vec<UserId>,
    remove_assignees: Vec<UserId>,
}

impl SourceMutation {
    fn is_empty(&self) -> bool {
        self.add_labels.is_empty()
            && self.remove_labels.is_empty()
            && self.add_assignees.is_empty()
            && self.remove_assignees.is_empty()
    }
}

fn claim_mutation(
    job: &InFlightJob,
    effects: &[Effect],
    include_label_effects: bool,
    current_role_user: Option<UserId>,
) -> SourceMutation {
    let mut mutation = SourceMutation::default();
    for effect in effects {
        match effect {
            Effect::AddLabel(label) if include_label_effects => {
                push_unique(&mut mutation.add_labels, label.as_str().to_string());
            }
            Effect::RemoveLabel(label) | Effect::RemoveLabelIfPresent(label)
                if include_label_effects =>
            {
                push_unique(&mut mutation.remove_labels, label.as_str().to_string());
            }
            Effect::SetAssignee(role) => {
                let user = resolve_effect_role_user(role, job, current_role_user.as_ref());
                push_unique(&mut mutation.add_assignees, user);
            }
            Effect::RemoveAssignee(role) => {
                let user = resolve_effect_role_user(role, job, current_role_user.as_ref());
                push_unique(&mut mutation.remove_assignees, user);
            }
            _ => {}
        }
    }
    mutation
}

fn source_claim_labels_still_current(
    labels: &[String],
    effects: &[Effect],
    artifact_kind: &ArtifactKindId,
    workflow: &ValidatedWorkflow,
) -> bool {
    let mut touched_labels = Vec::<String>::new();

    for effect in effects {
        match effect {
            Effect::AddLabel(label) => {
                let label = label.as_str();
                push_unique(&mut touched_labels, label.to_string());
                if !labels.iter().any(|existing| existing == label) {
                    return false;
                }
            }
            Effect::RemoveLabel(label) => {
                let label = label.as_str();
                push_unique(&mut touched_labels, label.to_string());
                if labels.iter().any(|existing| existing == label) {
                    return false;
                }
            }
            Effect::RemoveLabelIfPresent(label) => {
                push_unique(&mut touched_labels, label.as_str().to_string());
            }
            _ => {}
        }
    }

    // If any other workflow state label for this artifact is present, someone
    // has moved the item beyond the original claim. Do not project it back to
    // ready over that newer state.
    for dimension in workflow.state_dimensions() {
        for state in &dimension.states {
            let Some(label) = state.label.as_ref() else {
                continue;
            };
            if !state.allows_artifact(artifact_kind) {
                continue;
            }
            let label = label.as_str();
            if touched_labels.iter().any(|touched| touched == label) {
                continue;
            }
            if labels.iter().any(|existing| existing == label) {
                return false;
            }
        }
    }

    true
}

fn resolve_effect_role_user(
    role: &RoleId,
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

fn push_unique<T: Eq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}
