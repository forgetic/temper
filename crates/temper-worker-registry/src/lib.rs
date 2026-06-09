// SPDX-License-Identifier: MPL-2.0

//! Deterministic in-memory worker scheduling registry for the Temper daemon.
//!
//! The registry is a soft scheduling hint: it tracks worker capabilities,
//! health, and local in-flight capacity, but Forge leases/CAS remain the source
//! of truth for work ownership.

use std::collections::{BTreeMap, BTreeSet};

pub mod dispatch;
pub use dispatch::{Assignment, DispatchCoordinator, WorkItem};

use temper_worker_protocol::{Capability, Register};

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
        self.workers
            .iter()
            .filter(|(_, entry)| entry.healthy && entry.can_handle(role, repo))
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
        self.workers
            .get(worker_id)
            .is_some_and(|entry| entry.healthy && entry.can_handle(role, repo))
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
}

impl WorkerEntry {
    fn can_handle(&self, role: &str, repo: &str) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability.role == role && capability.repo == repo)
    }

    fn free_capacity(&self) -> u32 {
        self.max_concurrent_jobs
            .saturating_sub(self.in_flight.len().try_into().unwrap_or(u32::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_worker_protocol::{Capacity, WORKER_PROTOCOL_VERSION};

    fn register(worker_id: &str, role: &str, repo: &str, max_concurrent_jobs: u32) -> Register {
        Register {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.to_string(),
            capabilities: vec![Capability {
                role: role.to_string(),
                repo: repo.to_string(),
            }],
            capacity: Capacity {
                max_concurrent_jobs,
            },
            labels: None,
        }
    }

    #[test]
    fn register_then_assign_matches_capability() {
        let mut registry = WorkerRegistry::new();
        registry.register(&register("worker-a", "engineer", "ai/temper", 1));

        assert_eq!(
            registry.assign_candidate("engineer", "ai/temper"),
            Some("worker-a".to_string())
        );
    }

    #[test]
    fn can_handle_true_for_registered_capable_healthy_worker() {
        let mut registry = WorkerRegistry::new();
        registry.register(&register("worker-a", "engineer", "ai/temper", 0));

        assert!(registry.can_handle("worker-a", "engineer", "ai/temper"));
    }

    #[test]
    fn can_handle_false_for_wrong_role_or_repo() {
        let mut registry = WorkerRegistry::new();
        registry.register(&register("worker-a", "engineer", "ai/temper", 1));

        assert!(!registry.can_handle("worker-a", "architect", "ai/temper"));
        assert!(!registry.can_handle("worker-a", "engineer", "ai/smith"));
    }

    #[test]
    fn can_handle_false_for_unknown_or_unhealthy_worker() {
        let mut registry = WorkerRegistry::new();

        assert!(!registry.can_handle("missing", "engineer", "ai/temper"));

        registry.register(&register("worker-a", "engineer", "ai/temper", 1));
        registry.mark_unhealthy("worker-a");

        assert!(!registry.can_handle("worker-a", "engineer", "ai/temper"));
    }

    #[test]
    fn assign_returns_none_without_a_capable_worker() {
        let mut registry = WorkerRegistry::new();
        registry.register(&register("worker-a", "engineer", "ai/temper", 1));

        assert_eq!(registry.assign_candidate("reviewer", "ai/temper"), None);
        assert_eq!(registry.assign_candidate("engineer", "ai/smith"), None);
    }

    #[test]
    fn saturated_worker_is_not_a_candidate() {
        let mut registry = WorkerRegistry::new();
        registry.register(&register("worker-a", "engineer", "ai/temper", 1));
        registry.record_assignment("worker-a", "job-1").unwrap();

        assert_eq!(registry.assign_candidate("engineer", "ai/temper"), None);
    }

    #[test]
    fn assign_candidate_is_deterministic() {
        let mut registry = WorkerRegistry::new();
        registry.register(&register("worker-b", "engineer", "ai/temper", 2));
        registry.register(&register("worker-a", "engineer", "ai/temper", 1));

        assert_eq!(
            registry.assign_candidate("engineer", "ai/temper"),
            Some("worker-b".to_string())
        );

        registry.record_assignment("worker-b", "job-1").unwrap();
        assert_eq!(
            registry.assign_candidate("engineer", "ai/temper"),
            Some("worker-a".to_string())
        );
    }

    #[test]
    fn completing_a_job_frees_capacity() {
        let mut registry = WorkerRegistry::new();
        registry.register(&register("worker-a", "engineer", "ai/temper", 1));
        registry.record_assignment("worker-a", "job-1").unwrap();
        registry.complete_job("worker-a", "job-1").unwrap();

        assert_eq!(
            registry.assign_candidate("engineer", "ai/temper"),
            Some("worker-a".to_string())
        );
    }

    #[test]
    fn record_assignment_enforces_backpressure_and_validity() {
        let mut registry = WorkerRegistry::new();
        assert_eq!(
            registry.record_assignment("missing", "job-1"),
            Err(RegistryError::UnknownWorker("missing".to_string()))
        );

        registry.register(&register("worker-a", "engineer", "ai/temper", 1));
        registry.mark_unhealthy("worker-a");
        assert_eq!(
            registry.record_assignment("worker-a", "job-1"),
            Err(RegistryError::UnknownWorker("worker-a".to_string()))
        );

        registry.heartbeat("worker-a").unwrap();
        registry.record_assignment("worker-a", "job-1").unwrap();
        assert_eq!(
            registry.record_assignment("worker-a", "job-1"),
            Err(RegistryError::DuplicateJob("job-1".to_string()))
        );
        assert_eq!(
            registry.record_assignment("worker-a", "job-2"),
            Err(RegistryError::NoCapacity("worker-a".to_string()))
        );
    }

    #[test]
    fn mark_unhealthy_reclaims_jobs_and_excludes_from_assignment() {
        let mut registry = WorkerRegistry::new();
        registry.register(&register("worker-a", "engineer", "ai/temper", 2));
        registry.record_assignment("worker-a", "job-b").unwrap();
        registry.record_assignment("worker-a", "job-a").unwrap();

        assert_eq!(
            registry.mark_unhealthy("worker-a"),
            vec!["job-a".to_string(), "job-b".to_string()]
        );
        assert_eq!(registry.free_capacity("worker-a"), Some(2));
        assert_eq!(registry.assign_candidate("engineer", "ai/temper"), None);

        registry.heartbeat("worker-a").unwrap();
        assert_eq!(
            registry.assign_candidate("engineer", "ai/temper"),
            Some("worker-a".to_string())
        );
    }

    #[test]
    fn re_register_updates_capabilities_and_preserves_in_flight() {
        let mut registry = WorkerRegistry::new();
        registry.register(&register("worker-a", "engineer", "ai/temper", 2));
        registry.record_assignment("worker-a", "job-1").unwrap();
        registry.mark_unhealthy("worker-a");
        registry.register(&register("worker-a", "reviewer", "ai/smith", 2));

        assert_eq!(registry.free_capacity("worker-a"), Some(2));
        assert_eq!(registry.assign_candidate("engineer", "ai/temper"), None);
        assert_eq!(
            registry.assign_candidate("reviewer", "ai/smith"),
            Some("worker-a".to_string())
        );
        assert!(registry.is_healthy("worker-a"));

        registry.record_assignment("worker-a", "job-2").unwrap();
        registry.register(&register("worker-a", "engineer", "ai/temper", 3));
        assert_eq!(registry.free_capacity("worker-a"), Some(2));
    }

    #[test]
    fn complete_job_is_idempotent() {
        let mut registry = WorkerRegistry::new();
        registry.register(&register("worker-a", "engineer", "ai/temper", 1));

        registry.complete_job("worker-a", "missing").unwrap();
        registry.record_assignment("worker-a", "job-1").unwrap();
        registry.complete_job("worker-a", "job-1").unwrap();
        registry.complete_job("worker-a", "job-1").unwrap();

        assert_eq!(registry.free_capacity("worker-a"), Some(1));
    }
}
