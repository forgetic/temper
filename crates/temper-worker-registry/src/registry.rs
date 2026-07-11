// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, BTreeSet};

use temper_protocol_worker::{Capability, Register};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    UnknownWorker(String),
    NoCapacity(String),
    DuplicateJob(String),
    IneligibleWorker(String),
    RoleCapacity(String),
    WorkstreamConflict(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    EmptyWorkerId,
    EmptyPoolName,
    UnknownPool(String),
    PoolMissingCapacity(String),
    CapacityExceeded {
        pool: String,
        requested: u32,
        max: u32,
    },
    CapabilityOutsidePool {
        pool: String,
        role: String,
        repo: String,
    },
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistrationError::EmptyWorkerId => write!(f, "worker_id must not be empty"),
            RegistrationError::EmptyPoolName => write!(f, "worker_pool must not be empty"),
            RegistrationError::UnknownPool(pool) => write!(f, "unknown worker pool `{pool}`"),
            RegistrationError::PoolMissingCapacity(pool) => write!(
                f,
                "worker pool `{pool}` must set max_concurrent_jobs before workers can register to it"
            ),
            RegistrationError::CapacityExceeded {
                pool,
                requested,
                max,
            } => write!(
                f,
                "worker capacity {requested} exceeds worker pool `{pool}` max_concurrent_jobs {max}"
            ),
            RegistrationError::CapabilityOutsidePool { pool, role, repo } => write!(
                f,
                "worker capability `{repo}:{role}` is outside worker pool `{pool}` policy"
            ),
        }
    }
}

impl std::error::Error for RegistrationError {}

/// One resolved `[[worker.pools]]` registration policy as seen by the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerPoolPolicy {
    pub name: String,
    pub roles: Vec<String>,
    pub repos: Vec<String>,
    pub max_concurrent_jobs: Option<u32>,
}

impl WorkerPoolPolicy {
    pub fn new(
        name: impl Into<String>,
        roles: Vec<String>,
        repos: Vec<String>,
        max_concurrent_jobs: Option<u32>,
    ) -> Self {
        Self {
            name: name.into(),
            roles,
            repos,
            max_concurrent_jobs,
        }
    }

    fn permits(&self, capability: &Capability) -> bool {
        self.roles.iter().any(|role| role == &capability.role)
            && self.repos.iter().any(|repo| repo == &capability.repo)
    }
}

/// Name-indexed pool registration policies. Empty means only legacy no-pool
/// workers can register; a worker that names a pool must match a policy here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerPoolPolicies {
    pools: BTreeMap<String, WorkerPoolPolicy>,
}

impl WorkerPoolPolicies {
    pub fn new(policies: Vec<WorkerPoolPolicy>) -> Self {
        let pools = policies
            .into_iter()
            .map(|policy| (policy.name.clone(), policy))
            .collect();
        Self { pools }
    }

    pub fn is_empty(&self) -> bool {
        self.pools.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&WorkerPoolPolicy> {
        self.pools.get(name)
    }

    pub fn policies(&self) -> impl Iterator<Item = &WorkerPoolPolicy> {
        self.pools.values()
    }
}

impl From<Vec<WorkerPoolPolicy>> for WorkerPoolPolicies {
    fn from(policies: Vec<WorkerPoolPolicy>) -> Self {
        Self::new(policies)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerSnapshot {
    pub worker_id: String,
    pub worker_pool: Option<String>,
    pub capabilities: Vec<Capability>,
    pub max_concurrent_jobs: u32,
    pub free_capacity: u32,
    pub healthy: bool,
}

#[derive(Debug, Clone)]
struct WorkerEntry {
    worker_pool: Option<String>,
    capabilities: Vec<Capability>,
    max_concurrent_jobs: u32,
    in_flight: BTreeSet<String>,
    healthy: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WorkerRegistry {
    workers: BTreeMap<String, WorkerEntry>,
}

impl WorkerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, msg: &Register) {
        self.workers
            .entry(msg.worker_id.clone())
            .and_modify(|entry| {
                entry.worker_pool = msg.worker_pool.clone();
                entry.capabilities = msg.capabilities.clone();
                entry.max_concurrent_jobs = msg.capacity.max_concurrent_jobs;
                entry.healthy = true;
            })
            .or_insert_with(|| WorkerEntry {
                worker_pool: msg.worker_pool.clone(),
                capabilities: msg.capabilities.clone(),
                max_concurrent_jobs: msg.capacity.max_concurrent_jobs,
                in_flight: BTreeSet::new(),
                healthy: true,
            });
    }

    pub fn register_with_policies(
        &mut self,
        msg: &Register,
        policies: &WorkerPoolPolicies,
    ) -> Result<(), RegistrationError> {
        validate_registration(msg, policies)?;
        self.register(msg);
        Ok(())
    }

    pub fn assign_candidate(&self, role: &str, repo: &str) -> Option<String> {
        self.assign_candidate_all(role, &[repo.to_string()])
    }

    /// Like [`assign_candidate`](Self::assign_candidate) but for a coordinated
    /// multi-repo job: the chosen worker must hold `(role, repo)` for **every**
    /// repository in `repos` (ADR 0023). `repos` is expected to be non-empty.
    pub fn assign_candidate_all(&self, role: &str, repos: &[String]) -> Option<String> {
        self.workers
            .iter()
            .filter(|(_, entry)| entry.healthy && entry.can_handle_all(role, repos))
            .filter_map(|(worker_id, entry)| {
                let free_capacity = entry.free_capacity();
                (free_capacity > 0).then_some((worker_id, free_capacity))
            })
            .max_by(|(left_id, left_capacity), (right_id, right_capacity)| {
                left_capacity
                    .cmp(right_capacity)
                    .then_with(|| right_id.cmp(left_id))
            })
            .map(|(worker_id, _)| worker_id.clone())
    }

    /// True iff `worker_id` is registered, healthy, and its capabilities cover
    /// `(role, repo)`. Does not consider capacity.
    pub fn can_handle(&self, worker_id: &str, role: &str, repo: &str) -> bool {
        self.can_handle_all(worker_id, role, &[repo.to_string()])
    }

    /// True iff `worker_id` is registered, healthy, and its capabilities cover
    /// `(role, repo)` for **every** repository in `repos` (ADR 0023). Does not
    /// consider capacity.
    pub fn can_handle_all(&self, worker_id: &str, role: &str, repos: &[String]) -> bool {
        self.workers
            .get(worker_id)
            .is_some_and(|entry| entry.healthy && entry.can_handle_all(role, repos))
    }

    pub fn record_assignment(
        &mut self,
        worker_id: &str,
        job_id: &str,
    ) -> Result<(), RegistryError> {
        let entry = self
            .workers
            .get_mut(worker_id)
            .filter(|entry| entry.healthy)
            .ok_or_else(|| RegistryError::UnknownWorker(worker_id.to_string()))?;

        if entry.in_flight.contains(job_id) {
            return Err(RegistryError::DuplicateJob(job_id.to_string()));
        }

        if entry.free_capacity() == 0 {
            return Err(RegistryError::NoCapacity(worker_id.to_string()));
        }

        entry.in_flight.insert(job_id.to_string());
        Ok(())
    }

    /// Reconstitutes one durable in-flight assignment for its recorded worker.
    ///
    /// Recovery deliberately uses the same health and capacity checks as normal
    /// dispatch. It is idempotent for an already-restored `(worker, job)` pair,
    /// which lets repeated startup inventories and matching heartbeats converge
    /// without consuming a second slot.
    pub fn restore_assignment(
        &mut self,
        worker_id: &str,
        job_id: &str,
    ) -> Result<(), RegistryError> {
        let entry = self
            .workers
            .get_mut(worker_id)
            .filter(|entry| entry.healthy)
            .ok_or_else(|| RegistryError::UnknownWorker(worker_id.to_string()))?;

        if entry.in_flight.contains(job_id) {
            return Ok(());
        }

        if entry.free_capacity() == 0 {
            return Err(RegistryError::NoCapacity(worker_id.to_string()));
        }

        entry.in_flight.insert(job_id.to_string());
        Ok(())
    }

    pub fn complete_job(&mut self, worker_id: &str, job_id: &str) -> Result<(), RegistryError> {
        let entry = self
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| RegistryError::UnknownWorker(worker_id.to_string()))?;

        entry.in_flight.remove(job_id);
        Ok(())
    }

    pub fn heartbeat(&mut self, worker_id: &str) -> Result<(), RegistryError> {
        let entry = self
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| RegistryError::UnknownWorker(worker_id.to_string()))?;

        entry.healthy = true;
        Ok(())
    }

    pub fn mark_unhealthy(&mut self, worker_id: &str) -> Vec<String> {
        let Some(entry) = self.workers.get_mut(worker_id) else {
            return Vec::new();
        };

        entry.healthy = false;
        std::mem::take(&mut entry.in_flight).into_iter().collect()
    }

    pub fn worker_ids(&self) -> impl Iterator<Item = &str> {
        self.workers.keys().map(String::as_str)
    }

    pub fn free_capacity(&self, worker_id: &str) -> Option<u32> {
        self.workers.get(worker_id).map(WorkerEntry::free_capacity)
    }

    pub fn is_healthy(&self, worker_id: &str) -> bool {
        self.workers
            .get(worker_id)
            .is_some_and(|entry| entry.healthy)
    }

    pub fn worker_snapshots(&self) -> Vec<WorkerSnapshot> {
        self.workers
            .iter()
            .map(|(worker_id, entry)| WorkerSnapshot {
                worker_id: worker_id.clone(),
                worker_pool: entry.worker_pool.clone(),
                capabilities: entry.capabilities.clone(),
                max_concurrent_jobs: entry.max_concurrent_jobs,
                free_capacity: entry.free_capacity(),
                healthy: entry.healthy,
            })
            .collect()
    }

    /// Total registered workers, healthy or not. The `total` of the §4
    /// `{healthy, total}` worker tile.
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// Registered workers currently marked healthy. The `healthy` of the §4
    /// `{healthy, total}` worker tile.
    pub fn healthy_worker_count(&self) -> usize {
        self.workers.values().filter(|entry| entry.healthy).count()
    }
}

fn validate_registration(
    msg: &Register,
    policies: &WorkerPoolPolicies,
) -> Result<(), RegistrationError> {
    if msg.worker_id.trim().is_empty() {
        return Err(RegistrationError::EmptyWorkerId);
    }

    let Some(pool_name) = msg.worker_pool.as_deref() else {
        return Ok(());
    };
    if pool_name.trim().is_empty() {
        return Err(RegistrationError::EmptyPoolName);
    }

    let policy = policies
        .get(pool_name)
        .ok_or_else(|| RegistrationError::UnknownPool(pool_name.to_string()))?;
    let max = policy
        .max_concurrent_jobs
        .ok_or_else(|| RegistrationError::PoolMissingCapacity(pool_name.to_string()))?;
    if msg.capacity.max_concurrent_jobs > max {
        return Err(RegistrationError::CapacityExceeded {
            pool: pool_name.to_string(),
            requested: msg.capacity.max_concurrent_jobs,
            max,
        });
    }

    for capability in &msg.capabilities {
        if !policy.permits(capability) {
            return Err(RegistrationError::CapabilityOutsidePool {
                pool: pool_name.to_string(),
                role: capability.role.clone(),
                repo: capability.repo.clone(),
            });
        }
    }

    Ok(())
}

impl WorkerEntry {
    fn can_handle(&self, role: &str, repo: &str) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability.role == role && capability.repo == repo)
    }

    /// Covers `(role, repo)` for every repo in `repos`. A coordinated job's
    /// worker must be capable of all manifest repos, writable and read-only
    /// alike (ADR 0023). Empty `repos` is vacuously true; callers guarantee at
    /// least the primary repo is present.
    fn can_handle_all(&self, role: &str, repos: &[String]) -> bool {
        repos.iter().all(|repo| self.can_handle(role, repo))
    }

    fn free_capacity(&self) -> u32 {
        self.max_concurrent_jobs
            .saturating_sub(self.in_flight.len().try_into().unwrap_or(u32::MAX))
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod registry_tests;
