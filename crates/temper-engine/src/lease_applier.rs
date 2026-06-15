// SPDX-License-Identifier: MPL-2.0

//! Lease-gated [`ResultApplier`] decorator and the daemon wall-clock seam.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use temper_forge_model::{Forge, ItemNumber, RepositoryPath};
use temper_worker_protocol::JobProgress;
use temper_worker_protocol::JobResult;
use temper_workflow::{ArtifactSource, LeaseError, LeaseManager, LeasePolicy, RoleId};

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
pub struct LeaseApplier<F: Forge> {
    forge: Arc<F>,
    policy: LeasePolicy,
    owner: String,
    inner: Arc<dyn ResultApplier>,
    clock: WallClock,
}

impl<F: Forge> LeaseApplier<F> {
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
impl<F: Forge + 'static> ResultApplier for LeaseApplier<F> {
    /// Progress checkpoints are daemon-authored bookkeeping comments, not
    /// role-authored workflow mutations, so they bypass the lease gate.
    async fn apply_progress(&self, job: InFlightJob, progress: JobProgress) {
        self.inner.apply_progress(job, progress).await;
    }

    async fn apply(&self, job: InFlightJob, result: JobResult) {
        let Some((repo_id, target)) = resolve_target(self.forge.as_ref(), &job).await else {
            eprintln!(
                "temper-daemon: lease applier could not resolve target for job_id={} repo={} artifact.kind={} artifact.item={}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
            return;
        };

        let manager = LeaseManager::new(self.forge.as_ref(), self.policy);
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
            Ok(_) => {}
            Err(LeaseError::Conflict(_) | LeaseError::Contended { .. }) => return,
            Err(error) => {
                eprintln!(
                    "temper-daemon: lease applier could not acquire lease for job_id={} repo={} artifact.kind={} artifact.item={}: {error}",
                    job.job_id, job.repo, job.artifact.kind, job.artifact.item
                );
                return;
            }
        }

        self.inner.apply(job.clone(), result).await;

        if let Err(error) = manager.release(&repo_id, target, &self.owner).await {
            eprintln!(
                "temper-daemon: lease applier could not release lease for job_id={} repo={} artifact.kind={} artifact.item={}: {error}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
        }
    }
}

async fn resolve_target<F: Forge + ?Sized>(
    forge: &F,
    job: &InFlightJob,
) -> Option<(temper_forge_model::RepositoryId, ArtifactSource)> {
    let (owner, name) = job.repo.split_once('/')?;

    let repository = match forge
        .get_repository_by_path(&RepositoryPath::new(owner, name))
        .await
    {
        Ok(Some(repository)) => repository,
        Ok(None) => return None,
        Err(error) => {
            eprintln!(
                "temper-daemon: lease applier repository lookup failed for job_id={} repo={}: {error}",
                job.job_id, job.repo
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
