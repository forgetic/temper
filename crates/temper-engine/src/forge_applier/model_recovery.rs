// SPDX-License-Identifier: MPL-2.0

//! Model-recovery-specific Forge parking and safe audit projection.

use sha2::{Digest, Sha256};
use temper_forge::{
    CreateComment, Forge, Issue, PullRequest, UpdateIssue, UpdatePullRequest, UserId,
};
use temper_log::WorkItemRef;
use temper_log::emit::{ModelFailureParked, ModelRecoveryDecision, emit_model_failure_parked};
use temper_protocol_activity::ModelFailureV1;
use temper_protocol_worker::{
    FailureClass, JobContext, JobResult, SessionRecoveryActionV1, SessionRecoveryEvidenceV1,
};
use temper_workflow::{AssignmentMutation, Effect};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;

const MODEL_RECOVERY_AUDIT_KEY_PREFIX: &str = "model_recovery_park:";

pub(super) struct ModelRecoveryParkEvidence {
    diagnostic: ModelFailureV1,
    recovery: SessionRecoveryEvidenceV1,
}

impl ModelRecoveryParkEvidence {
    pub(super) fn from_result(result: &JobResult) -> Option<Self> {
        let failure = result.failure.as_ref()?;
        let recovery = failure.session_recovery.as_ref()?;
        if failure.class != FailureClass::Permanent
            || recovery.action != SessionRecoveryActionV1::ParkForHuman
            || recovery
                .validate_for_attempt(result.attempt_id.as_deref())
                .is_err()
        {
            return None;
        }
        let mut diagnostic = failure.model_failure.clone()?;
        diagnostic.normalize();
        Some(Self {
            diagnostic,
            recovery: recovery.clone(),
        })
    }
}

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn park_model_recovery(
        &self,
        job: &InFlightJob,
        result: &JobResult,
        evidence: &ModelRecoveryParkEvidence,
    ) -> Result<(), String> {
        let target = match job.artifact.kind.as_str() {
            "issue" => self
                .resolve_issue(job)
                .await
                .map(|(_, issue)| ParkTarget::Issue(Box::new(issue))),
            "pull_request" => self
                .resolve_pull_request(job)
                .await
                .map(|(_, pull)| ParkTarget::PullRequest(Box::new(pull))),
            other => return Err(format!("cannot park unsupported artifact kind `{other}`")),
        }
        .ok_or_else(|| "could not resolve model-failure source artifact".to_string())?;

        let marker = model_recovery_comment_marker(job, evidence.recovery.failure_epoch);
        let comments = target
            .list_comments(self.forge.as_ref())
            .await
            .map_err(|error| format!("list model-recovery audit comments: {error}"))?;
        let has_comment = comments
            .iter()
            .any(|comment| comment.body.contains(&marker));

        let mutation = self.model_recovery_park_mutation(job, &target).await?;
        if mutation_has_changes(&mutation) {
            target
                .update(self.forge.as_ref(), mutation)
                .await
                .map_err(|error| {
                    format!("converge model-recovery park labels/assignee: {error}")
                })?;
        }
        if !has_comment {
            target
                .add_comment(
                    self.forge.as_ref(),
                    model_recovery_audit_body(evidence, &marker),
                )
                .await
                .map_err(|error| format!("add model-recovery audit comment: {error}"))?;
        }

        let item = target.work_item(&job.repo);
        emit_model_failure_parked(ModelFailureParked {
            item: &item,
            worker_id: &result.worker_id,
            job_id: &result.job_id,
            decision: ModelRecoveryDecision {
                attempt_id: &evidence.recovery.attempt_id,
                failure_epoch: evidence.recovery.failure_epoch,
                failure_count: evidence.recovery.failure_count,
                action: evidence.recovery.action.as_str(),
                current_session_id: &evidence.recovery.current_session_id,
                prior_session_id: evidence.recovery.prior_session_id.as_deref(),
                new_session_id: evidence.recovery.new_session_id.as_deref(),
                evidence_location: &evidence.recovery.evidence_location,
                model_failure: &evidence.diagnostic,
            },
        });
        Ok(())
    }

    async fn model_recovery_park_mutation(
        &self,
        job: &InFlightJob,
        target: &ParkTarget,
    ) -> Result<AssignmentMutation, String> {
        let current_user = self
            .forge
            .current_user()
            .await
            .map_err(|error| format!("resolve active role assignee for model park: {error}"))?;
        let mut mutation = AssignmentMutation::default();
        for label in &self.attention_labels {
            if !target.labels().iter().any(|existing| existing == label) {
                push_unique(&mut mutation.add_labels, label.clone());
            }
        }
        // These are presentation/queue labels, never artifact-kind or history labels.
        push_unique(&mut mutation.remove_labels, "ready".to_string());
        push_unique(&mut mutation.remove_labels, "in-progress".to_string());
        push_unique(&mut mutation.remove_assignees, current_user.id.clone());
        // Claims made while current-user lookup was unavailable use the role id.
        push_unique(
            &mut mutation.remove_assignees,
            UserId::new(job.role.as_str()),
        );

        if let Some(effects) = self.action_effects(job) {
            for effect in effects {
                match effect {
                    Effect::RemoveLabel(label) | Effect::RemoveLabelIfPresent(label) => {
                        push_unique(&mut mutation.remove_labels, label.as_str().to_string());
                    }
                    Effect::AddLabel(label) if self.is_working_label(&label) => {
                        push_unique(&mut mutation.remove_labels, label.as_str().to_string());
                    }
                    Effect::SetAssignee(role) => {
                        push_unique(
                            &mut mutation.remove_assignees,
                            if role.as_str() == job.role {
                                current_user.id.clone()
                            } else {
                                UserId::new(role.as_str())
                            },
                        );
                    }
                    _ => {}
                }
            }
        }

        mutation
            .remove_labels
            .retain(|label| target.labels().iter().any(|existing| existing == label));
        mutation
            .remove_assignees
            .retain(|user| target.assignees().iter().any(|existing| existing == user));
        Ok(mutation)
    }
}

enum ParkTarget {
    Issue(Box<Issue>),
    PullRequest(Box<PullRequest>),
}

impl ParkTarget {
    fn labels(&self) -> &[String] {
        match self {
            Self::Issue(issue) => &issue.labels,
            Self::PullRequest(pull) => &pull.labels,
        }
    }

    fn assignees(&self) -> &[UserId] {
        match self {
            Self::Issue(issue) => &issue.assignees,
            Self::PullRequest(pull) => &pull.assignees,
        }
    }

    fn work_item(&self, repo: &str) -> WorkItemRef {
        match self {
            Self::Issue(issue) => WorkItemRef::issue(repo, issue.number.get()),
            Self::PullRequest(pull) => WorkItemRef::pull_request(repo, pull.number.get()),
        }
    }

    async fn list_comments<F: Forge + ?Sized>(
        &self,
        forge: &F,
    ) -> temper_forge::ForgeResult<Vec<temper_forge::Comment>> {
        match self {
            Self::Issue(issue) => forge.list_issue_comments(&issue.id).await,
            Self::PullRequest(pull) => forge.list_pull_request_comments(&pull.id).await,
        }
    }

    async fn update<F: Forge + ?Sized>(
        &self,
        forge: &F,
        mutation: AssignmentMutation,
    ) -> temper_forge::ForgeResult<()> {
        match self {
            Self::Issue(issue) => forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        add_labels: mutation.add_labels,
                        remove_labels: mutation.remove_labels,
                        add_assignees: mutation.add_assignees,
                        remove_assignees: mutation.remove_assignees,
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
                .map(|_| ()),
            Self::PullRequest(pull) => forge
                .update_pull_request(
                    &pull.id,
                    UpdatePullRequest {
                        add_labels: mutation.add_labels,
                        remove_labels: mutation.remove_labels,
                        add_assignees: mutation.add_assignees,
                        remove_assignees: mutation.remove_assignees,
                        expected_version: Some(pull.version),
                        ..UpdatePullRequest::default()
                    },
                )
                .await
                .map(|_| ()),
        }
    }

    async fn add_comment<F: Forge + ?Sized>(
        &self,
        forge: &F,
        body: String,
    ) -> temper_forge::ForgeResult<()> {
        let input = CreateComment { body };
        match self {
            Self::Issue(issue) => forge.add_issue_comment(&issue.id, input).await.map(|_| ()),
            Self::PullRequest(pull) => forge
                .add_pull_request_comment(&pull.id, input)
                .await
                .map(|_| ()),
        }
    }
}

fn model_recovery_comment_marker(job: &InFlightJob, failure_epoch: u32) -> String {
    let workstream = serde_json::from_value::<JobContext>(job.job_payload.clone())
        .ok()
        .and_then(|context| context.workspace)
        .map(|workspace| workspace.coordination_key)
        .unwrap_or_else(|| format!("{}:{}:{}", job.repo, job.artifact.kind, job.artifact.item));
    let digest = Sha256::digest(workstream.as_bytes());
    format!(
        "<!-- temper:comment-key={MODEL_RECOVERY_AUDIT_KEY_PREFIX}{digest:x}:{failure_epoch} -->"
    )
}

fn model_recovery_audit_body(evidence: &ModelRecoveryParkEvidence, marker: &str) -> String {
    let recovery = &evidence.recovery;
    let failure = &evidence.diagnostic;
    let optional = |value: Option<&str>| html_code(value.unwrap_or("none"));
    format!(
        "Temper parked this workstream because bounded model recovery was exhausted. Automatic claims are disabled until an operator explicitly restores queue eligibility.\n\n\
**Durable recovery decision**\n\
- attempt_id: {}\n\
- failure_epoch: `{}`\n\
- failure_count: `{}`\n\
- action: `{}`\n\
- current_session_id (failed fresh session): {}\n\
- prior_session_id: {}\n\
- new_session_id: {}\n\
- evidence_location: {}\n\n\
**Safe model diagnostic**\n\
- provider: {}\n\
- model: {}\n\
- category: `{}`\n\
- retryable: `{}`\n\
- http_status: `{}`\n\
- provider_request_id: {}\n\
- provider_error_code: {}\n\
- detail_redacted: `{}`\n\
- message: {}\n\n\
**Operator action:** inspect the preserved workspace and session ledger at the evidence location, resolve the provider/session problem without discarding workspace changes, then deliberately restore the appropriate queue label.\n\n{}",
        html_code(&recovery.attempt_id),
        recovery.failure_epoch,
        recovery.failure_count,
        recovery.action.as_str(),
        html_code(&recovery.current_session_id),
        optional(recovery.prior_session_id.as_deref()),
        optional(recovery.new_session_id.as_deref()),
        html_code(&recovery.evidence_location),
        html_code(&failure.provider),
        html_code(&failure.model),
        failure.category.as_str(),
        failure.retryable,
        failure
            .http_status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "none".to_string()),
        optional(failure.provider_request_id.as_deref()),
        optional(failure.provider_error_code.as_deref()),
        failure.detail_redacted,
        html_code(&failure.message),
        marker,
    )
}

fn html_code(value: &str) -> String {
    let escaped = value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("<code>{escaped}</code>")
}

fn mutation_has_changes(mutation: &AssignmentMutation) -> bool {
    !mutation.add_labels.is_empty()
        || !mutation.remove_labels.is_empty()
        || !mutation.add_assignees.is_empty()
        || !mutation.remove_assignees.is_empty()
}

fn push_unique<T: Eq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}
