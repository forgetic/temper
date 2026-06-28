// SPDX-License-Identifier: MPL-2.0

//! Lease-gated [`ResultApplier`] decorator and the daemon wall-clock seam.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use temper_forge::{Forge, ItemNumber, RepositoryPath};
use temper_log::emit::{
    LeaseClaimed, LeaseLost, LeaseReleased, emit_lease_claimed, emit_lease_lost,
    emit_lease_released,
};
use temper_log::{WorkItemRef, format_duration, strip_provider_scheme, work_item_span};
use temper_protocol_worker::JobResult;
use temper_protocol_worker::{PullRequestFreshness, PullRequestFreshnessResponse};
use temper_workflow::{ArtifactSource, LeaseError, LeaseManager, LeasePolicy, RoleId};
use tracing::Instrument;

use crate::InFlightJob;
use crate::applier::ResultApplier;

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
/// The decorator resolves the completed worker job's Forge artifact, acquires
/// the workflow lease for that `(artifact, role)` as the daemon owner, invokes
/// the inner applier only while that lease is held, and then releases the lease
/// best-effort. Duplicate or double-dispatched results that lose the lease race
/// no-op without disturbing the peer's live lease.
pub struct LeaseApplier<F: Forge + ?Sized> {
    forge: Arc<F>,
    policy: LeasePolicy,
    owner: String,
    inner: Arc<dyn ResultApplier>,
    clock: WallClock,
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
            owner: owner.into(),
            inner,
            clock,
        }
    }
}

#[async_trait::async_trait]
impl<F: Forge + ?Sized + 'static> ResultApplier for LeaseApplier<F> {
    async fn check_pull_request_freshness(
        &self,
        check: PullRequestFreshness,
    ) -> PullRequestFreshnessResponse {
        self.inner.check_pull_request_freshness(check).await
    }

    async fn apply(&self, job: InFlightJob, result: JobResult) {
        let Some((repo_id, target)) = resolve_target(self.forge.as_ref(), &job).await else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                "lease applier could not resolve target"
            );
            return;
        };

        // §7 work-item ref for the lease lifecycle lines; the bare owner/repo is
        // already scheme-free, but strip defensively to share the helper.
        let item = lease_item_ref(&job.repo, target);
        let ttl = self.policy.ttl();
        let ttl_human = format_duration(ttl.to_std().unwrap_or_default());
        let ttl_ms = u64::try_from(ttl.num_milliseconds()).unwrap_or(0);

        let manager = LeaseManager::new(self.forge.as_ref(), self.policy);

        // §4a per-work-item span: opened on lease claim and closed on completion,
        // so the acquire → inner apply → release sequence (and every event the
        // inner applier emits in between) inherits `artifact.ref`/`repo`/`role`.
        // The "apply result" running label is the only transition-like name at
        // this seam, matching the lease-claimed line. Instrumenting the future
        // (rather than holding an `.entered()` guard across the awaits below)
        // keeps the span correctly scoped across await points.
        let span = work_item_span(&item, &job.role, Some("apply result"));
        async move {
            match manager
                .acquire(
                    &repo_id,
                    target,
                    RoleId::new(job.role.clone()),
                    self.owner.clone(),
                    (self.clock)(),
                )
                .await
            {
                Ok(_) => {
                    emit_lease_claimed(LeaseClaimed {
                        item: &item,
                        role: &job.role,
                        ttl_human: &ttl_human,
                        ttl_ms,
                        running: "apply result",
                    });
                }
                Err(LeaseError::Conflict(_) | LeaseError::Contended { .. }) => {
                    emit_lease_lost(LeaseLost {
                        item: &item,
                        role: &job.role,
                        reason: "contended by peer owner",
                    });
                    return;
                }
                Err(error) => {
                    emit_lease_lost(LeaseLost {
                        item: &item,
                        role: &job.role,
                        reason: "acquire failed",
                    });
                    tracing::error!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        repo = %job.repo,
                        artifact_kind = %job.artifact.kind,
                        artifact_item = %job.artifact.item,
                        %error,
                        "lease applier could not acquire lease"
                    );
                    return;
                }
            }

            self.inner.apply(job.clone(), result).await;

            if let Err(error) = manager.release(&repo_id, target, &self.owner).await {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    artifact_kind = %job.artifact.kind,
                    artifact_item = %job.artifact.item,
                    %error,
                    "lease applier could not release lease"
                );
            }
            emit_lease_released(LeaseReleased {
                item: &item,
                role: &job.role,
            });
        }
        .instrument(span)
        .await;
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
