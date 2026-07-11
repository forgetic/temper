// SPDX-License-Identifier: MPL-2.0

//! Serializable DTOs over the daemon's in-memory dispatch state.
//!
//! These are thin `#[derive(Serialize)]` projections built from the (non-
//! serializable) internal `QueuedJob` / `InFlightJob` / `Artifact` types so the
//! `GET /v1/state` endpoint can answer a JSON snapshot without leaking internal
//! types or deriving `Serialize` on them.
//!
//! This is the **raw** daemon snapshot — the lowest-level honest view of
//! dispatch state. temper-web (PR D) projects it into the board's card model.
//! The field set here is sufficient for that projection; the mapping to the
//! board's cold-start contract fixture
//! (`crates/temper-web/ui/fixtures/state-snapshot.json`,
//! `{ workers:{healthy,total,registered?}, cards:{ id:{id,lane,ref,role,title,...} } }`) is:
//!
//! - `workers.healthy` / `workers.total` -> the fixture's `workers` tile (1:1),
//!   with `workers.registered` carrying the raw worker id, pool, capacity, and
//!   capabilities for operator/debug views.
//! - each queued/in-flight job -> one board card, keyed by `artifact_ref`
//!   (the `ref` join key, e.g. `acme/widgets#42` / `acme/widgets PR#8`), which
//!   is the same `artifact.ref` the board joins log deltas onto (UX Appendix
//!   A.1/A.2). `role` carries straight across. The board derives `lane` from
//!   workflow state dimensions and `title` from the forge (UX Appendix A.4) —
//!   those are PR D's projection, not raw daemon state, so they are absent here.
//! - `queued` vs `in_flight` distinguishes pending work from dispatched work, so
//!   the board can seed a coarse pending/running lane before richer lane
//!   derivation lands.
//! - `role_saturation` surfaces the §7 saturated-role wait lists for the
//!   attention/problem overlay.

use serde::Serialize;
use temper_log::{WorkItemRef, strip_provider_scheme};
use temper_protocol_worker::{Artifact, Capability};
use temper_worker_registry::daemon_core::QueuedJob;
use temper_worker_registry::{DaemonCore, InFlightJob, WorkerSnapshot};

/// Healthy / total worker tile plus raw registered-worker details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkersDto {
    pub healthy: usize,
    pub total: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registered: Vec<WorkerDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkerCapabilityDto {
    pub role: String,
    pub repo: String,
}

impl From<&Capability> for WorkerCapabilityDto {
    fn from(capability: &Capability) -> Self {
        Self {
            role: capability.role.clone(),
            repo: capability.repo.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkerDto {
    pub worker_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
    pub healthy: bool,
    pub max_concurrent_jobs: u32,
    pub free_capacity: u32,
    pub capabilities: Vec<WorkerCapabilityDto>,
}

impl From<&WorkerSnapshot> for WorkerDto {
    fn from(worker: &WorkerSnapshot) -> Self {
        Self {
            worker_id: worker.worker_id.clone(),
            pool: worker.worker_pool.clone(),
            healthy: worker.healthy,
            max_concurrent_jobs: worker.max_concurrent_jobs,
            free_capacity: worker.free_capacity,
            capabilities: worker
                .capabilities
                .iter()
                .map(WorkerCapabilityDto::from)
                .collect(),
        }
    }
}

/// The artifact an issue/PR job coordinates, as wire JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArtifactDto {
    /// Raw artifact identity preserved from the protocol (string or number).
    pub item: serde_json::Value,
    /// Artifact kind, e.g. `issue` or `pull_request`.
    pub kind: String,
}

impl From<&Artifact> for ArtifactDto {
    fn from(artifact: &Artifact) -> Self {
        Self {
            item: artifact.item.clone(),
            kind: artifact.kind.clone(),
        }
    }
}

/// One dispatch job (queued or in-flight) in raw snapshot form.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobDto {
    pub job_id: String,
    pub role: String,
    /// Bare `owner/repo` path of the coordinating artifact's home repository.
    pub repo: String,
    pub artifact: ArtifactDto,
    /// Canonical `artifact.ref` join key (`owner/repo#n` / `owner/repo PR#n`),
    /// or `null` when the artifact identity is non-numeric or its kind is
    /// unrecognised. This is the key the board projects cards on and joins log
    /// deltas onto.
    #[serde(rename = "ref")]
    pub artifact_ref: Option<String>,
}

impl JobDto {
    /// Serialize to a `serde_json::Value` for the HTTP response body.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("JobDto serializes")
    }

    fn new(job_id: &str, role: &str, repo: &str, artifact: &Artifact) -> Self {
        Self {
            job_id: job_id.to_string(),
            role: role.to_string(),
            repo: repo.to_string(),
            artifact: ArtifactDto::from(artifact),
            artifact_ref: artifact_ref_string(repo, artifact),
        }
    }
}

impl From<&QueuedJob> for JobDto {
    fn from(job: &QueuedJob) -> Self {
        Self::new(&job.job_id, &job.role, &job.repo, &job.artifact)
    }
}

impl From<&InFlightJob> for JobDto {
    fn from(job: &InFlightJob) -> Self {
        Self::new(&job.job_id, &job.role, &job.repo, &job.artifact)
    }
}

/// A saturated role's pending wait list: jobs queued at its configured limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoleSaturationDto {
    pub role: String,
    /// The role's configured finite concurrency limit.
    pub concurrency: usize,
    /// `artifact.ref` strings of same-role jobs waiting at the limit.
    pub waiting: Vec<String>,
}

/// The full raw daemon dispatch snapshot served by `GET /v1/state`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DaemonStateSnapshot {
    pub workers: WorkersDto,
    pub queued: Vec<JobDto>,
    pub in_flight: Vec<JobDto>,
    pub role_saturation: Vec<RoleSaturationDto>,
}

impl DaemonStateSnapshot {
    /// Read the snapshot from live daemon state. Pure in-process reads over the
    /// single-threaded daemon core — no locking, no I/O.
    pub fn from_core(core: &DaemonCore) -> Self {
        let registry = core.coordinator().registry();
        let worker_snapshots = registry.worker_snapshots();
        let queued_jobs = core.queued_jobs();
        let queued: Vec<JobDto> = queued_jobs.iter().map(JobDto::from).collect();
        let in_flight: Vec<JobDto> = core
            .coordinator()
            .assigned_work_items()
            .filter_map(|item| core.in_flight_job(&item.job_id))
            .map(|job| JobDto::from(&job))
            .collect();
        let role_saturation = role_saturation_dtos(core, &queued_jobs);

        Self {
            workers: WorkersDto {
                healthy: registry.healthy_worker_count(),
                total: registry.worker_count(),
                registered: worker_snapshots.iter().map(WorkerDto::from).collect(),
            },
            queued,
            in_flight,
            role_saturation,
        }
    }

    /// Serialize to a `serde_json::Value` for the HTTP response body.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("DaemonStateSnapshot serializes")
    }
}

/// Builds the saturated-role wait lists, one per distinct role with pending
/// same-role work at its configured finite limit.
fn role_saturation_dtos(core: &DaemonCore, queued: &[QueuedJob]) -> Vec<RoleSaturationDto> {
    let mut roles: Vec<String> = queued.iter().map(|job| job.role.clone()).collect();
    roles.sort();
    roles.dedup();

    roles
        .into_iter()
        .filter_map(|role| {
            let saturation = core.role_saturation(&role)?;
            let waiting: Vec<String> = saturation
                .pending
                .iter()
                .filter_map(|(repo, artifact)| artifact_ref_string(repo, artifact))
                .collect();
            if waiting.is_empty() {
                return None;
            }
            Some(RoleSaturationDto {
                concurrency: saturation.concurrency as usize,
                role,
                waiting,
            })
        })
        .collect()
}

/// Canonical `artifact.ref` join key for a job, mirroring the daemon's existing
/// `role.saturated` wait-list formatter (handlers.rs). `None` for a non-numeric
/// item or an unrecognised kind.
fn artifact_ref_string(repo: &str, artifact: &Artifact) -> Option<String> {
    let number = artifact.item.as_u64()?;
    let repo = strip_provider_scheme(repo);
    let item = match artifact.kind.as_str() {
        "issue" => WorkItemRef::issue(repo, number),
        "pull_request" => WorkItemRef::pull_request(repo, number),
        _ => return None,
    };
    Some(item.to_string())
}

#[cfg(test)]
#[path = "state_dto_tests.rs"]
mod state_dto_tests;
