// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, BTreeSet};

use temper_protocol_worker::{Capability, Register};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    UnknownWorker(String),
    NoCapacity(String),
    DuplicateJob(String),
}

#[derive(Debug, Clone)]
struct WorkerEntry {
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
                entry.capabilities = msg.capabilities.clone();
                entry.max_concurrent_jobs = msg.capacity.max_concurrent_jobs;
                entry.healthy = true;
            })
            .or_insert_with(|| WorkerEntry {
                capabilities: msg.capabilities.clone(),
                max_concurrent_jobs: msg.capacity.max_concurrent_jobs,
                in_flight: BTreeSet::new(),
                healthy: true,
            });
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

    pub fn free_capacity(&self, worker_id: &str) -> Option<u32> {
        self.workers.get(worker_id).map(WorkerEntry::free_capacity)
    }

    pub fn is_healthy(&self, worker_id: &str) -> bool {
        self.workers
            .get(worker_id)
            .is_some_and(|entry| entry.healthy)
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
