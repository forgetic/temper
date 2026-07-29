// SPDX-License-Identifier: MPL-2.0

//! Durable provider-deferral projection and success publication authority.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use temper_forge::{
    CreateComment, Forge, ForgeError, Issue, PullRequest, UpdateIssue, UpdatePullRequest, UserId,
};
use temper_protocol_activity::{ModelFailureDispositionV1, ModelFailureV1};
use temper_protocol_worker::{
    FailureClass, JobContext, JobResult, SessionRecoveryActionV1, SessionRecoveryEvidenceV1,
};
use temper_workflow::{
    AssignmentMutation, Effect, ProviderRecovery, ProviderRecoveryDisposition,
    ProviderRecoveryFacts, parse_metadata_block, replace_metadata_block,
};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;

pub(super) struct ModelRecoveryDeferralEvidence {
    diagnostic: ModelFailureV1,
    recovery: SessionRecoveryEvidenceV1,
}

impl ModelRecoveryDeferralEvidence {
    pub(super) fn from_result(result: &JobResult) -> Option<Self> {
        let failure = result.failure.as_ref()?;
        let recovery = failure.session_recovery.as_ref()?;
        if failure.class != FailureClass::Transient
            || recovery.action != SessionRecoveryActionV1::ProviderDeferred
            || recovery
                .validate_for_attempt(result.attempt_id.as_deref())
                .is_err()
            || recovery.epoch_started_unix_ms.is_none()
            || recovery.not_before_unix_ms.is_none()
            || recovery.slo_deadline_unix_ms.is_none()
        {
            return None;
        }
        let mut diagnostic = failure.model_failure.clone()?;
        diagnostic.normalize();
        if diagnostic.validate().is_err()
            || !matches!(
                diagnostic.disposition,
                ModelFailureDispositionV1::Retryable | ModelFailureDispositionV1::Unknown
            )
            || recovery.disposition != Some(diagnostic.disposition)
        {
            return None;
        }
        Some(Self {
            diagnostic,
            recovery: recovery.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SuccessRecoveryAuthority {
    Authorized,
    Stale,
    Corrupt,
}

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn defer_model_recovery(
        &self,
        job: &InFlightJob,
        result: &JobResult,
        evidence: &ModelRecoveryDeferralEvidence,
    ) -> Result<(), String> {
        let context = serde_json::from_value::<JobContext>(job.job_payload.clone())
            .map_err(|error| format!("invalid provider-deferral JobContext: {error}"))?;
        let workstream = context
            .workspace
            .as_ref()
            .map(|workspace| workspace.coordination_key.trim())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "provider deferral requires a coordination-scoped workspace".to_string()
            })?;
        let target = match job.artifact.kind.as_str() {
            "issue" => self
                .resolve_issue(job)
                .await
                .map(|(_, issue)| RecoveryTarget::Issue(Box::new(issue))),
            "pull_request" => self
                .resolve_pull_request(job)
                .await
                .map(|(_, pull)| RecoveryTarget::PullRequest(Box::new(pull))),
            _ => None,
        }
        .ok_or_else(|| "could not resolve provider-deferral source artifact".to_string())?;

        let desired = provider_recovery_from_evidence(
            workstream,
            result.attempt_id.as_deref(),
            &evidence.diagnostic,
            &evidence.recovery,
            None,
        )?;
        self.converge_provider_deferral(job, result, target, desired)
            .await
    }

    async fn converge_provider_deferral(
        &self,
        job: &InFlightJob,
        result: &JobResult,
        mut target: RecoveryTarget,
        mut desired: ProviderRecovery,
    ) -> Result<(), String> {
        for _ in 0..3 {
            let mut metadata = parse_metadata_block(target.body())
                .map_err(|error| format!("parse provider-deferral metadata: {error}"))?
                .unwrap_or_default();
            let assignment = metadata
                .assignment
                .as_ref()
                .ok_or_else(|| "provider deferral lost its exact durable assignment".to_string())?
                .clone();
            if assignment.job_id.as_deref() != Some(job.job_id.as_str())
                || assignment.attempt_id.as_deref() != result.attempt_id.as_deref()
                || assignment.worker_id.as_deref() != Some(result.worker_id.as_str())
                || assignment.coordination_key.as_deref() != Some(desired.workstream_id.as_str())
            {
                return Err("provider deferral does not own the fresh exact assignment".to_string());
            }

            if let Some(current) = metadata.provider_recovery.as_ref() {
                current
                    .validate()
                    .map_err(|reason| format!("corrupt durable provider recovery: {reason}"))?;
                if current.source_attempt_id == desired.source_attempt_id
                    && current.failure_epoch == desired.failure_epoch
                {
                    // Duplicate result delivery converges presentation only.
                    desired = current.as_ref().clone();
                } else {
                    if !current.authorizes_attempt(result.attempt_id.as_deref())
                        || current.workstream_id != desired.workstream_id
                        || current.failure_epoch != desired.failure_epoch
                        || desired.deferral_count <= current.deferral_count
                    {
                        return Err(
                            "provider deferral update is stale or outside its due generation"
                                .to_string(),
                        );
                    }
                    desired.generation = desired.generation.max(
                        current
                            .generation
                            .checked_add(1)
                            .ok_or_else(|| "provider recovery generation overflow".to_string())?,
                    );
                    desired.idempotency_key = recovery_event_key(
                        &desired.workstream_id,
                        desired.failure_epoch,
                        desired.generation,
                        &desired.source_attempt_id,
                        "defer",
                    );
                }
            }
            desired.due_assignment_attempt_id = None;
            desired.health_event_id = None;
            desired.validate()?;
            metadata.provider_recovery = Some(Box::new(desired.clone()));
            let body = replace_metadata_block(target.body(), &metadata)
                .map_err(|error| format!("render provider-deferral metadata: {error}"))?;
            let mutation = self.provider_deferral_mutation(job, &target, &assignment);
            match target.update(self.forge.as_ref(), body, mutation).await {
                Ok(_) => return Ok(()),
                Err(ForgeError::Conflict(_)) => {
                    target = self
                        .reload_recovery_target(job)
                        .await
                        .ok_or_else(|| "provider-deferral target disappeared".to_string())?;
                }
                Err(error) => return Err(format!("persist provider deferral: {error}")),
            }
        }
        Err("provider-deferral convergence remained contended".to_string())
    }

    pub(super) async fn success_recovery_authority(
        &self,
        job: &InFlightJob,
    ) -> SuccessRecoveryAuthority {
        let target = match job.artifact.kind.as_str() {
            "issue" => self
                .resolve_issue(job)
                .await
                .map(|(_, issue)| RecoveryTarget::Issue(Box::new(issue))),
            "pull_request" => self
                .resolve_pull_request(job)
                .await
                .map(|(_, pull)| RecoveryTarget::PullRequest(Box::new(pull))),
            _ => None,
        };
        let Some(target) = target else {
            return SuccessRecoveryAuthority::Stale;
        };
        let metadata = match parse_metadata_block(target.body()) {
            Ok(metadata) => metadata.unwrap_or_default(),
            Err(_) if crate::provider_recovery::body_has_provider_recovery_key(target.body()) => {
                return SuccessRecoveryAuthority::Corrupt;
            }
            Err(_) => return SuccessRecoveryAuthority::Authorized,
        };
        let Some(recovery) = metadata.provider_recovery.as_ref() else {
            return SuccessRecoveryAuthority::Authorized;
        };
        if recovery.validate().is_err() {
            return SuccessRecoveryAuthority::Corrupt;
        }
        let assignment_matches = metadata.assignment.as_ref().is_some_and(|assignment| {
            assignment.job_id.as_deref() == Some(job.job_id.as_str())
                && assignment.attempt_id.as_deref() == job.attempt_id.as_deref()
                && assignment.coordination_key.as_deref() == Some(recovery.workstream_id.as_str())
        });
        if assignment_matches {
            if recovery.authorizes_attempt(job.attempt_id.as_deref()) {
                SuccessRecoveryAuthority::Authorized
            } else {
                SuccessRecoveryAuthority::Corrupt
            }
        } else {
            SuccessRecoveryAuthority::Stale
        }
    }

    pub(super) async fn park_corrupt_provider_recovery(
        &self,
        job: &InFlightJob,
    ) -> Result<(), String> {
        let mut target = self
            .reload_recovery_target(job)
            .await
            .ok_or_else(|| "provider-recovery source artifact disappeared".to_string())?;
        let marker = format!(
            "<!-- temper:comment-key=provider_recovery_corrupt:{} -->",
            recovery_event_key(&job.job_id, 1, 1, "corrupt", "park")
        );
        let comments = target
            .comments(self.forge.as_ref())
            .await
            .map_err(|error| format!("list corrupt provider-recovery audits: {error}"))?;
        let mut mutation = AssignmentMutation::default();
        if !target.labels().iter().any(|label| label == "needs-human") {
            mutation.add_labels.push("needs-human".to_string());
        }
        for label in ["ready", "in-progress"] {
            if target.labels().iter().any(|current| current == label) {
                mutation.remove_labels.push(label.to_string());
            }
        }
        mutation.remove_assignees = target.assignees().to_vec();
        if mutation_has_changes(&mutation) {
            target = target
                .update(self.forge.as_ref(), target.body().to_string(), mutation)
                .await
                .map_err(|error| format!("park corrupt provider recovery: {error}"))?;
        }
        if !comments
            .iter()
            .any(|comment| comment.body.contains(&marker))
        {
            target
                .comment(
                    self.forge.as_ref(),
                    format!(
                        "Temper parked this workstream because its durable provider recovery fence is corrupt.\n\n**Operator repair:** inspect the preserved workspace and session ledger, repair or remove the bounded `provider_recovery` record, then deliberately restore queue eligibility.\n\n{marker}"
                    ),
                )
                .await
                .map_err(|error| format!("audit corrupt provider recovery: {error}"))?;
        }
        Ok(())
    }

    async fn reload_recovery_target(&self, job: &InFlightJob) -> Option<RecoveryTarget> {
        match job.artifact.kind.as_str() {
            "issue" => self
                .resolve_issue(job)
                .await
                .map(|(_, issue)| RecoveryTarget::Issue(Box::new(issue))),
            "pull_request" => self
                .resolve_pull_request(job)
                .await
                .map(|(_, pull)| RecoveryTarget::PullRequest(Box::new(pull))),
            _ => None,
        }
    }

    fn provider_deferral_mutation(
        &self,
        job: &InFlightJob,
        target: &RecoveryTarget,
        assignment: &temper_workflow::DurableAssignment,
    ) -> AssignmentMutation {
        let mut mutation = AssignmentMutation::default();
        if let Some(effects) = self.action_effects(job) {
            for effect in effects {
                match effect {
                    Effect::AddLabel(label)
                        if self.is_working_label(&label)
                            && !assignment
                                .pre_claim_labels
                                .iter()
                                .any(|current| current == label.as_str()) =>
                    {
                        push_unique(&mut mutation.remove_labels, label.as_str().to_string());
                    }
                    Effect::RemoveLabel(label) | Effect::RemoveLabelIfPresent(label)
                        if assignment
                            .pre_claim_labels
                            .iter()
                            .any(|current| current == label.as_str()) =>
                    {
                        push_unique(&mut mutation.add_labels, label.as_str().to_string());
                    }
                    _ => {}
                }
            }
        }
        mutation
            .add_labels
            .retain(|label| !target.labels().contains(label));
        mutation
            .remove_labels
            .retain(|label| target.labels().contains(label));
        // Restore the pre-claim assignment set exactly. A queue may carry
        // unrelated human/watch assignees; deferral releases only ownership
        // introduced by this claim and must not erase those durable actors.
        let pre_claim_assignees = assignment
            .pre_claim_assignees
            .iter()
            .map(|assignee| UserId::new(assignee.clone()))
            .collect::<Vec<_>>();
        mutation.add_assignees = pre_claim_assignees
            .iter()
            .filter(|assignee| !target.assignees().contains(assignee))
            .cloned()
            .collect();
        mutation.remove_assignees = target
            .assignees()
            .iter()
            .filter(|assignee| !pre_claim_assignees.contains(assignee))
            .cloned()
            .collect();
        mutation
    }
}

fn provider_recovery_from_evidence(
    workstream: &str,
    attempt_id: Option<&str>,
    diagnostic: &ModelFailureV1,
    recovery: &SessionRecoveryEvidenceV1,
    minimum_generation: Option<u32>,
) -> Result<ProviderRecovery, String> {
    let source_attempt_id = attempt_id
        .filter(|attempt| !attempt.trim().is_empty())
        .ok_or_else(|| "provider deferral requires an exact attempt id".to_string())?;
    let disposition = match diagnostic.disposition {
        ModelFailureDispositionV1::Retryable => ProviderRecoveryDisposition::Retryable,
        ModelFailureDispositionV1::Unknown => ProviderRecoveryDisposition::Unknown,
        ModelFailureDispositionV1::NonRetryable => {
            return Err("non-retryable failure cannot create provider deferral".to_string());
        }
    };
    let generation = recovery
        .deferral_generation
        .max(minimum_generation.unwrap_or_default());
    let epoch_started_at = timestamp(recovery.epoch_started_unix_ms, "epoch start")?;
    let not_before = timestamp(recovery.not_before_unix_ms, "not-before")?;
    let slo_deadline = timestamp(recovery.slo_deadline_unix_ms, "SLO deadline")?;
    let marker = ProviderRecovery {
        workstream_id: workstream.to_string(),
        failure_epoch: recovery.failure_epoch,
        disposition,
        facts: ProviderRecoveryFacts {
            provider: diagnostic.provider.clone(),
            model: diagnostic.model.clone(),
            category: diagnostic.category.as_str().to_string(),
            boundary: diagnostic.boundary.as_str().to_string(),
            event_kind: diagnostic.event_kind.as_str().to_string(),
            status_present: diagnostic.status_present,
            code_present: diagnostic.code_present,
            http_status: diagnostic.http_status,
            provider_request_id: diagnostic.provider_request_id.clone(),
            provider_error_code: diagnostic.provider_error_code.clone(),
        },
        cumulative_failure_count: recovery.failure_count,
        deferral_count: recovery.deferral_count,
        deferral_limit: recovery.configured_deferral_limit,
        generation,
        not_before,
        epoch_started_at,
        elapsed_ms: recovery.epoch_elapsed_ms,
        slo_deadline,
        idempotency_key: recovery_event_key(
            workstream,
            recovery.failure_epoch,
            generation,
            source_attempt_id,
            "defer",
        ),
        source_attempt_id: source_attempt_id.to_string(),
        due_assignment_attempt_id: None,
        health_event_id: None,
    };
    marker.validate()?;
    Ok(marker)
}

fn timestamp(value: Option<u64>, field: &str) -> Result<DateTime<Utc>, String> {
    let value = value.ok_or_else(|| format!("provider deferral omitted {field}"))?;
    let millis = i64::try_from(value).map_err(|_| format!("provider deferral {field} overflow"))?;
    DateTime::from_timestamp_millis(millis)
        .ok_or_else(|| format!("provider deferral {field} is outside the timestamp range"))
}

pub(crate) fn recovery_event_key(
    workstream: &str,
    epoch: u32,
    generation: u32,
    event_id: &str,
    kind: &str,
) -> String {
    let mut digest = Sha256::new();
    for value in [
        workstream,
        &epoch.to_string(),
        &generation.to_string(),
        event_id,
        kind,
    ] {
        digest.update(value.len().to_string());
        digest.update(b":");
        digest.update(value.as_bytes());
    }
    format!("{:x}", digest.finalize())
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

enum RecoveryTarget {
    Issue(Box<Issue>),
    PullRequest(Box<PullRequest>),
}

impl RecoveryTarget {
    fn body(&self) -> &str {
        match self {
            Self::Issue(issue) => &issue.body,
            Self::PullRequest(pull) => &pull.body,
        }
    }

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

    async fn comments<F: Forge + ?Sized>(
        &self,
        forge: &F,
    ) -> temper_forge::ForgeResult<Vec<temper_forge::Comment>> {
        match self {
            Self::Issue(issue) => forge.list_issue_comments(&issue.id).await,
            Self::PullRequest(pull) => forge.list_pull_request_comments(&pull.id).await,
        }
    }

    async fn comment<F: Forge + ?Sized>(
        &self,
        forge: &F,
        body: String,
    ) -> temper_forge::ForgeResult<()> {
        match self {
            Self::Issue(issue) => forge
                .add_issue_comment(&issue.id, CreateComment { body })
                .await
                .map(|_| ()),
            Self::PullRequest(pull) => forge
                .add_pull_request_comment(&pull.id, CreateComment { body })
                .await
                .map(|_| ()),
        }
    }

    async fn update<F: Forge + ?Sized>(
        &self,
        forge: &F,
        body: String,
        mutation: AssignmentMutation,
    ) -> temper_forge::ForgeResult<Self> {
        match self {
            Self::Issue(issue) => forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        body: Some(body),
                        add_labels: mutation.add_labels,
                        remove_labels: mutation.remove_labels,
                        add_assignees: mutation.add_assignees,
                        remove_assignees: mutation.remove_assignees,
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
                .map(|issue| Self::Issue(Box::new(issue))),
            Self::PullRequest(pull) => forge
                .update_pull_request(
                    &pull.id,
                    UpdatePullRequest {
                        body: Some(body),
                        add_labels: mutation.add_labels,
                        remove_labels: mutation.remove_labels,
                        add_assignees: mutation.add_assignees,
                        remove_assignees: mutation.remove_assignees,
                        expected_version: Some(pull.version),
                        ..UpdatePullRequest::default()
                    },
                )
                .await
                .map(|pull| Self::PullRequest(Box::new(pull))),
        }
    }
}
