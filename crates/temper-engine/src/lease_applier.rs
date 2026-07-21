// SPDX-License-Identifier: MPL-2.0

//! Lease-gated [`ResultApplier`] decorator and the daemon wall-clock seam.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use temper_forge::{Forge, ForgeResult, ItemNumber, RepositoryId, RepositoryPath};
use temper_log::emit::{LeaseLost, LeaseReleased, emit_lease_lost, emit_lease_released};
use temper_log::{WorkItemRef, strip_provider_scheme, work_item_span};
use temper_protocol_worker::{
    JobContext, JobResult, PullRequestFreshness, PullRequestFreshnessResponse,
    PullRequestFreshnessStatus,
};
use temper_workflow::{
    ArtifactSource, AssignmentClaimRequest, DurableAssignment, LeaseError, LeaseManager,
    LeasePolicy, RecoveredHeartbeatOutcome, RecoveredOwnershipLossReason, RoleId,
};
use tracing::Instrument;

use crate::InFlightJob;
use crate::applier::{ApplyOutcome, ClaimContext, ClaimOutcome, ResultApplier};

/// Wall-clock capability for daemon code needing calendar timestamps (lease
/// acquisition, scan feeds). Always injected — production passes
/// [`system_clock`], the simulation passes a virtual-time-derived clock —
/// so daemon-owned loops never read ambient wall time.
pub type WallClock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// The production wall clock.
pub fn system_clock() -> WallClock {
    Arc::new(Utc::now)
}

type AttemptKey = (String, Option<String>);

#[derive(Default)]
struct LocalAuthority {
    claims: BTreeMap<AttemptKey, ClaimContext>,
    /// Monotonic within this process: a definitively lost exact attempt can
    /// never be restored by a delayed heartbeat.
    revoked: BTreeSet<AttemptKey>,
}

/// Lease-gated [`ResultApplier`] decorator for daemon-owned result application.
///
/// The decorator resolves the job's Forge artifact and persists the exact
/// assignment identity, lease, and lifecycle mutation before publication. On
/// result delivery it validates that same job/worker claim, invokes the inner
/// applier without reacquiring under another owner, and releases the durable
/// claim after terminal or retry bookkeeping.
pub struct LeaseApplier<F: Forge + ?Sized> {
    forge: Arc<F>,
    policy: LeasePolicy,
    _owner: String,
    inner: Arc<dyn ResultApplier>,
    clock: WallClock,
    authority: Mutex<LocalAuthority>,
}

impl<F: Forge + ?Sized> LeaseApplier<F> {
    pub fn new(
        forge: Arc<F>,
        policy: LeasePolicy,
        owner: impl Into<String>,
        inner: Arc<dyn ResultApplier>,
        clock: WallClock,
    ) -> Self {
        Self {
            forge,
            policy,
            _owner: owner.into(),
            inner,
            clock,
            authority: Mutex::new(LocalAuthority::default()),
        }
    }
}

#[async_trait::async_trait]
impl<F: Forge + ?Sized + 'static> ResultApplier for LeaseApplier<F> {
    async fn claim(&self, job: InFlightJob, context: ClaimContext) -> ClaimOutcome {
        if self.is_revoked(&job) {
            return ClaimOutcome::Stale {
                reason: "exact assignment attempt was already revoked in this process".to_string(),
            };
        }
        if let Some(outcome) = self.validate_claim_freshness(&job).await {
            return outcome;
        }
        let (repo_id, target) = match resolve_target(self.forge.as_ref(), &job).await {
            Ok(Some(target)) => target,
            Ok(None) => {
                return ClaimOutcome::Stale {
                    reason: "assignment target no longer exists".to_string(),
                };
            }
            Err(error) => {
                report_target_lookup_failure(&job, "claim", &error);
                return ClaimOutcome::Retryable {
                    reason: format!("could not resolve assignment target: {error}"),
                };
            }
        };
        let assignment = durable_assignment(&job, &context);
        let mutation = self.inner.assignment_mutation(&job).await;
        let manager = LeaseManager::new(self.forge.as_ref(), self.policy);
        match manager
            .claim_assignment(
                &repo_id,
                target,
                AssignmentClaimRequest {
                    assignment,
                    mutation,
                },
                (self.clock)(),
            )
            .await
        {
            Ok(_) => {
                let key = attempt_key(&job);
                let authorized = {
                    let mut authority = self.authority.lock().expect("assignment authority lock");
                    if authority.revoked.contains(&key) {
                        false
                    } else {
                        authority.claims.insert(key, context.clone());
                        true
                    }
                };
                if authorized {
                    self.inner.claim(job, context).await
                } else {
                    ClaimOutcome::Stale {
                        reason: "exact assignment attempt was revoked while claiming".to_string(),
                    }
                }
            }
            Err(
                LeaseError::Conflict(_)
                | LeaseError::Contended { .. }
                | LeaseError::AssignmentConflict { .. },
            ) => ClaimOutcome::Contended {
                reason: "assignment claim was won by another daemon".to_string(),
            },
            Err(LeaseError::TargetMissing { .. }) => ClaimOutcome::Stale {
                reason: "assignment target no longer exists".to_string(),
            },
            Err(error) => ClaimOutcome::Retryable {
                reason: format!("could not persist assignment claim: {error}"),
            },
        }
    }

    async fn release_claim(&self, job: InFlightJob, context: ClaimContext) {
        self.authority
            .lock()
            .expect("assignment authority lock")
            .claims
            .remove(&attempt_key(&job));
        let (repo_id, target) = match resolve_target(self.forge.as_ref(), &job).await {
            Ok(Some(target)) => target,
            Ok(None) => {
                tracing::warn!(
                    target: "temper_daemon",
                    operation = "release",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    artifact_kind = %job.artifact.kind,
                    artifact_item = %job.artifact.item,
                    "lease applier assignment target no longer exists; durable assignment cleanup deferred to lease expiry and live reconciliation"
                );
                return;
            }
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    operation = "release",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    artifact_kind = %job.artifact.kind,
                    artifact_item = %job.artifact.item,
                    %error,
                    "lease applier repository lookup failed; durable assignment cleanup deferred to lease expiry and live reconciliation"
                );
                return;
            }
        };
        let expected = durable_assignment(&job, &context);
        let manager = LeaseManager::new(self.forge.as_ref(), self.policy);
        if let Err(error) = manager
            .rollback_assignment(&repo_id, target, &expected)
            .await
        {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                %error,
                "could not release unpublished durable assignment"
            );
        }
    }

    async fn check_pull_request_freshness(
        &self,
        check: PullRequestFreshness,
    ) -> PullRequestFreshnessResponse {
        self.inner.check_pull_request_freshness(check).await
    }

    async fn heartbeat(
        &self,
        job: InFlightJob,
        context: ClaimContext,
    ) -> RecoveredHeartbeatOutcome {
        if self.is_revoked(&job) {
            return RecoveredHeartbeatOutcome::OwnershipLost {
                reason: RecoveredOwnershipLossReason::AssignmentReplaced,
            };
        }
        let (repo_id, target) = match resolve_target(self.forge.as_ref(), &job).await {
            Ok(Some(target)) => target,
            Ok(None) => {
                tracing::warn!(
                    target: "temper_daemon",
                    operation = "heartbeat",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    artifact_kind = %job.artifact.kind,
                    artifact_item = %job.artifact.item,
                    "recovered assignment heartbeat target no longer exists"
                );
                let outcome = RecoveredHeartbeatOutcome::OwnershipLost {
                    reason: RecoveredOwnershipLossReason::TargetRemoved,
                };
                self.revoke(&job);
                return outcome;
            }
            Err(error) => {
                report_target_lookup_failure(&job, "heartbeat", &error);
                return RecoveredHeartbeatOutcome::TransientlyUnavailable {
                    reason: format!("could not resolve recovered assignment target: {error}"),
                };
            }
        };
        let outcome = self
            .reattach_recovered_claim(&job, &context, &repo_id, target)
            .await;
        match &outcome {
            RecoveredHeartbeatOutcome::Owned => {}
            RecoveredHeartbeatOutcome::TransientlyUnavailable { reason } => {
                tracing::warn!(
                    target: "temper_daemon",
                    operation = "heartbeat",
                    job_id = %job.job_id,
                    reason,
                    "recovered assignment heartbeat is transiently unavailable"
                );
            }
            RecoveredHeartbeatOutcome::OwnershipLost { reason } => {
                tracing::warn!(
                    target: "temper_daemon",
                    operation = "heartbeat",
                    job_id = %job.job_id,
                    %reason,
                    "recovered assignment heartbeat lost durable ownership"
                );
            }
        }
        outcome
    }

    async fn apply_recovered(
        &self,
        job: InFlightJob,
        result: JobResult,
        context: ClaimContext,
    ) -> ApplyOutcome {
        let (repo_id, target) = match resolve_target(self.forge.as_ref(), &job).await {
            Ok(Some(target)) => target,
            Ok(None) => {
                tracing::warn!(
                    target: "temper_daemon",
                    operation = "apply",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    artifact_kind = %job.artifact.kind,
                    artifact_item = %job.artifact.item,
                    "lease applier assignment target no longer exists"
                );
                self.revoke(&job);
                return ApplyOutcome::Stale;
            }
            Err(error) => {
                report_target_lookup_failure(&job, "apply", &error);
                return ApplyOutcome::Retryable {
                    reason: format!("could not resolve recovered assignment target: {error}"),
                };
            }
        };
        match self
            .reattach_recovered_claim(&job, &context, &repo_id, target)
            .await
        {
            RecoveredHeartbeatOutcome::Owned => {}
            RecoveredHeartbeatOutcome::TransientlyUnavailable { reason } => {
                tracing::error!(
                    target: "temper_daemon",
                    operation = "apply",
                    job_id = %job.job_id,
                    reason,
                    "lease applier could not verify recovered result claim"
                );
                return ApplyOutcome::Retryable {
                    reason: format!("could not verify recovered result claim: {reason}"),
                };
            }
            RecoveredHeartbeatOutcome::OwnershipLost { reason } => {
                tracing::warn!(
                    target: "temper_daemon",
                    operation = "apply",
                    job_id = %job.job_id,
                    %reason,
                    "lease applier recovered result no longer owns durable claim"
                );
                return ApplyOutcome::Stale;
            }
        }
        self.apply(job, result).await
    }

    async fn apply(&self, job: InFlightJob, result: JobResult) -> ApplyOutcome {
        let Some(claim_context) = self
            .authority
            .lock()
            .expect("assignment authority lock")
            .claims
            .get(&attempt_key(&job))
            .cloned()
        else {
            return ApplyOutcome::Stale;
        };
        if claim_context.worker_id != result.worker_id || job.attempt_id != result.attempt_id {
            return ApplyOutcome::Stale;
        }
        let (repo_id, target) = match resolve_target(self.forge.as_ref(), &job).await {
            Ok(Some(target)) => target,
            Ok(None) => {
                tracing::warn!(
                    target: "temper_daemon",
                    operation = "apply",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    artifact_kind = %job.artifact.kind,
                    artifact_item = %job.artifact.item,
                    "lease applier assignment target no longer exists"
                );
                self.revoke(&job);
                return ApplyOutcome::Stale;
            }
            Err(error) => {
                report_target_lookup_failure(&job, "apply", &error);
                return ApplyOutcome::Retryable {
                    reason: format!("could not resolve assignment target: {error}"),
                };
            }
        };

        // §7 work-item ref for the lease lifecycle lines; the bare owner/repo is
        // already scheme-free, but strip defensively to share the helper.
        let item = lease_item_ref(&job.repo, target);
        let manager = LeaseManager::new(self.forge.as_ref(), self.policy);
        let expected = durable_assignment(&job, &claim_context);

        // Result application consumes the assignment-time lease instead of
        // reacquiring under a possibly conflicting daemon owner.
        let span = work_item_span(&item, &job.role, Some("apply result"));
        async move {
            match manager
                .validate_assignment(&repo_id, target, &expected)
                .await
            {
                Ok(true) => {}
                Ok(false) | Err(LeaseError::AssignmentConflict { .. }) => {
                    emit_lease_lost(LeaseLost {
                        item: &item,
                        role: &job.role,
                        reason: "durable assignment did not match result",
                    });
                    self.revoke(&job);
                    return ApplyOutcome::Stale;
                }
                Err(LeaseError::TargetMissing { .. } | LeaseError::MalformedMetadata { .. }) => {
                    emit_lease_lost(LeaseLost {
                        item: &item,
                        role: &job.role,
                        reason: "durable assignment state was removed or malformed",
                    });
                    self.revoke(&job);
                    return ApplyOutcome::Stale;
                }
                Err(error) => {
                    tracing::error!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        %error,
                        "lease applier could not validate durable assignment"
                    );
                    return ApplyOutcome::Retryable {
                        reason: format!("could not validate durable assignment: {error}"),
                    };
                }
            }

            let outcome = self.inner.apply(job.clone(), result).await;
            if matches!(
                outcome,
                ApplyOutcome::Retryable { .. } | ApplyOutcome::ConvergencePending { .. }
            ) {
                return outcome;
            }

            if let Err(error) = manager
                .release_assignment(&repo_id, target, &expected)
                .await
            {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    %error,
                    "lease applier could not release durable assignment"
                );
                return ApplyOutcome::Retryable {
                    reason: format!("could not release durable assignment: {error}"),
                };
            } else {
                emit_lease_released(LeaseReleased {
                    item: &item,
                    role: &job.role,
                });
            }
            self.authority
                .lock()
                .expect("assignment authority lock")
                .claims
                .remove(&attempt_key(&job));
            outcome
        }
        .instrument(span)
        .await
    }
}

impl<F: Forge + ?Sized> LeaseApplier<F> {
    async fn reattach_recovered_claim(
        &self,
        job: &InFlightJob,
        context: &ClaimContext,
        repo_id: &RepositoryId,
        target: ArtifactSource,
    ) -> RecoveredHeartbeatOutcome {
        if self.is_revoked(job) {
            return RecoveredHeartbeatOutcome::OwnershipLost {
                reason: RecoveredOwnershipLossReason::AssignmentReplaced,
            };
        }
        let expected = durable_assignment(job, context);
        let manager = LeaseManager::new(self.forge.as_ref(), self.policy);
        let outcome = manager
            .heartbeat_assignment(repo_id, target, &expected, (self.clock)())
            .await;
        match outcome {
            RecoveredHeartbeatOutcome::Owned => {
                let key = attempt_key(job);
                let mut authority = self.authority.lock().expect("assignment authority lock");
                if authority.revoked.contains(&key) {
                    RecoveredHeartbeatOutcome::OwnershipLost {
                        reason: RecoveredOwnershipLossReason::AssignmentReplaced,
                    }
                } else {
                    authority.claims.insert(key, context.clone());
                    RecoveredHeartbeatOutcome::Owned
                }
            }
            RecoveredHeartbeatOutcome::OwnershipLost { reason } => {
                self.revoke(job);
                RecoveredHeartbeatOutcome::OwnershipLost { reason }
            }
            transient => transient,
        }
    }

    fn is_revoked(&self, job: &InFlightJob) -> bool {
        self.authority
            .lock()
            .expect("assignment authority lock")
            .revoked
            .contains(&attempt_key(job))
    }

    fn revoke(&self, job: &InFlightJob) {
        let key = attempt_key(job);
        let mut authority = self.authority.lock().expect("assignment authority lock");
        authority.revoked.insert(key.clone());
        authority.claims.remove(&key);
    }

    async fn validate_claim_freshness(&self, job: &InFlightJob) -> Option<ClaimOutcome> {
        let check = serde_json::from_value::<JobContext>(job.job_payload.clone())
            .ok()?
            .pull_request_freshness?;
        let response = self.inner.check_pull_request_freshness(check).await;
        let reason = response
            .reason
            .unwrap_or_else(|| "pull request freshness check gave no reason".to_string());
        match response.status {
            PullRequestFreshnessStatus::Fresh => None,
            PullRequestFreshnessStatus::Stale => Some(ClaimOutcome::Stale { reason }),
            PullRequestFreshnessStatus::Unavailable => Some(ClaimOutcome::Retryable { reason }),
        }
    }
}

fn attempt_key(job: &InFlightJob) -> AttemptKey {
    (job.job_id.clone(), job.attempt_id.clone())
}

fn durable_assignment(job: &InFlightJob, context: &ClaimContext) -> DurableAssignment {
    let parsed = serde_json::from_value::<JobContext>(job.job_payload.clone()).ok();
    DurableAssignment {
        job_id: Some(job.job_id.clone()),
        attempt_id: job.attempt_id.clone(),
        role: Some(RoleId::new(job.role.clone())),
        queue: parsed.as_ref().map(|context| context.queue.clone()),
        action: parsed.as_ref().and_then(|context| context.action.clone()),
        worker_id: Some(context.worker_id.clone()),
        coordination_key: parsed
            .as_ref()
            .and_then(|context| context.workspace.as_ref())
            .map(|workspace| workspace.coordination_key.clone()),
        daemon_boot_id: Some(context.daemon_boot_id.clone()),
        assignment_pr_head: parsed
            .as_ref()
            .and_then(|context| context.pull_request_freshness.as_ref())
            .and_then(|freshness| freshness.head_sha.clone()),
        ..DurableAssignment::default()
    }
}

/// Builds the §7 `artifact.ref` join key for a lease lifecycle line.
///
/// `repo` is the job's bare `owner/repo` path (the daemon already split it from
/// the provider id); [`strip_provider_scheme`] is a defensive no-op on that
/// shape and keeps the conversion identical to the runner's.
fn lease_item_ref(repo: &str, target: ArtifactSource) -> WorkItemRef {
    let repo = strip_provider_scheme(repo);
    match target {
        ArtifactSource::Issue { number } => WorkItemRef::issue(repo, number.get()),
        ArtifactSource::PullRequest { number } => WorkItemRef::pull_request(repo, number.get()),
    }
}

async fn resolve_target<F: Forge + ?Sized>(
    forge: &F,
    job: &InFlightJob,
) -> ForgeResult<Option<(RepositoryId, ArtifactSource)>> {
    let Some((owner, name)) = job.repo.split_once('/') else {
        return Ok(None);
    };
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Ok(None);
    }
    let Some(number) = job.artifact.item.as_u64().map(ItemNumber::new) else {
        return Ok(None);
    };
    let target = match job.artifact.kind.as_str() {
        "issue" => ArtifactSource::Issue { number },
        "pull_request" => ArtifactSource::PullRequest { number },
        _ => return Ok(None),
    };

    let Some(repository) = forge
        .get_repository_by_path(&RepositoryPath::new(owner, name))
        .await?
    else {
        return Ok(None);
    };

    Ok(Some((repository.id, target)))
}

fn report_target_lookup_failure(
    job: &InFlightJob,
    operation: &'static str,
    error: &temper_forge::ForgeError,
) {
    tracing::error!(
        target: "temper_daemon",
        operation,
        job_id = %job.job_id,
        repo = %job.repo,
        artifact_kind = %job.artifact.kind,
        artifact_item = %job.artifact.item,
        %error,
        "lease applier repository lookup failed"
    );
}
