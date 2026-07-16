// SPDX-License-Identifier: MPL-2.0

//! Lease-gated [`ResultApplier`] decorator and the daemon wall-clock seam.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use temper_forge::{Forge, ItemNumber, RepositoryPath};
use temper_log::emit::{LeaseLost, LeaseReleased, emit_lease_lost, emit_lease_released};
use temper_log::{WorkItemRef, strip_provider_scheme, work_item_span};
use temper_protocol_worker::{
    JobContext, JobResult, PullRequestFreshness, PullRequestFreshnessResponse,
    PullRequestFreshnessStatus,
};
use temper_workflow::{
    ArtifactSource, AssignmentClaimRequest, DurableAssignment, LeaseError, LeaseManager,
    LeasePolicy, RoleId,
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
    claims: Mutex<BTreeMap<String, ClaimContext>>,
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
            claims: Mutex::new(BTreeMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl<F: Forge + ?Sized + 'static> ResultApplier for LeaseApplier<F> {
    async fn claim(&self, job: InFlightJob, context: ClaimContext) -> ClaimOutcome {
        if let Some(outcome) = self.validate_claim_freshness(&job).await {
            return outcome;
        }
        let Some((repo_id, target)) = resolve_target(self.forge.as_ref(), &job).await else {
            return ClaimOutcome::Stale {
                reason: "assignment target no longer exists".to_string(),
            };
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
                self.claims
                    .lock()
                    .expect("assignment claim lock")
                    .insert(job.job_id.clone(), context.clone());
                self.inner.claim(job, context).await
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
        self.claims
            .lock()
            .expect("assignment claim lock")
            .remove(&job.job_id);
        let Some((repo_id, target)) = resolve_target(self.forge.as_ref(), &job).await else {
            return;
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

    async fn heartbeat(&self, job: InFlightJob, context: ClaimContext) {
        if let Err(error) = self.reattach_recovered_claim(&job, &context).await {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                %error,
                "recovered assignment heartbeat did not match durable claim"
            );
        }
    }

    async fn apply_recovered(
        &self,
        job: InFlightJob,
        result: JobResult,
        context: ClaimContext,
    ) -> ApplyOutcome {
        if let Err(error) = self.reattach_recovered_claim(&job, &context).await {
            tracing::error!(
                target: "temper_daemon",
                job_id = %job.job_id,
                %error,
                "lease applier could not reattach recovered result claim"
            );
            return ApplyOutcome::Retryable {
                reason: format!("could not reattach recovered result claim: {error}"),
            };
        }
        self.apply(job, result).await
    }

    async fn apply(&self, job: InFlightJob, result: JobResult) -> ApplyOutcome {
        let Some(claim_context) = self
            .claims
            .lock()
            .expect("assignment claim lock")
            .get(&job.job_id)
            .cloned()
        else {
            return ApplyOutcome::Stale;
        };
        if claim_context.worker_id != result.worker_id || job.attempt_id != result.attempt_id {
            return ApplyOutcome::Stale;
        }
        let Some((repo_id, target)) = resolve_target(self.forge.as_ref(), &job).await else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                "lease applier could not resolve target"
            );
            return ApplyOutcome::Stale;
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
            if matches!(outcome, ApplyOutcome::Retryable { .. }) {
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
            self.claims
                .lock()
                .expect("assignment claim lock")
                .remove(&job.job_id);
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
    ) -> Result<(), String> {
        let Some((repo_id, target)) = resolve_target(self.forge.as_ref(), job).await else {
            return Err("assignment target could not be resolved".to_string());
        };
        let expected = durable_assignment(job, context);
        let manager = LeaseManager::new(self.forge.as_ref(), self.policy);
        manager
            .heartbeat_assignment(&repo_id, target, &expected, (self.clock)())
            .await
            .map_err(|error| error.to_string())?;
        self.claims
            .lock()
            .expect("assignment claim lock")
            .insert(job.job_id.clone(), context.clone());
        Ok(())
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
) -> Option<(temper_forge::RepositoryId, ArtifactSource)> {
    let (owner, name) = job.repo.split_once('/')?;

    let repository = match forge
        .get_repository_by_path(&RepositoryPath::new(owner, name))
        .await
    {
        Ok(Some(repository)) => repository,
        Ok(None) => return None,
        Err(error) => {
            tracing::error!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                %error,
                "lease applier repository lookup failed"
            );
            return None;
        }
    };

    let number = job.artifact.item.as_u64().map(ItemNumber::new)?;
    let target = match job.artifact.kind.as_str() {
        "issue" => ArtifactSource::Issue { number },
        "pull_request" => ArtifactSource::PullRequest { number },
        _ => return None,
    };

    Some((repository.id, target))
}
