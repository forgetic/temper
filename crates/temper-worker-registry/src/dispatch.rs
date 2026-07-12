// SPDX-License-Identifier: MPL-2.0

//! Pure in-memory dispatch coordination over the soft worker registry.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use temper_protocol_worker::Register;

use crate::{RegistryError, WorkerRegistry};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkItem {
    pub job_id: String,
    pub role: String,
    /// Optional logical workstream key. When present, only one in-flight job for
    /// the same `(role, coordination_key)` may hold worker capacity at a time,
    /// even if distinct queue scans produced different job ids.
    pub coordination_key: Option<String>,
    /// Primary repository (home of the coordinating artifact) — carried on the
    /// resulting [`Assignment`] and the `Assign` envelope.
    pub repo: String,
    /// Every repository the assigned worker must be capable of: the primary
    /// plus any additional manifest repos of a coordinated job (ADR 0023).
    /// Always non-empty and includes `repo`.
    pub repos: Vec<String>,
}

impl WorkItem {
    /// A single-repo work item — the degenerate one-repo manifest.
    pub fn single(
        job_id: impl Into<String>,
        role: impl Into<String>,
        repo: impl Into<String>,
    ) -> Self {
        let repo = repo.into();
        Self {
            job_id: job_id.into(),
            role: role.into(),
            coordination_key: None,
            repos: vec![repo.clone()],
            repo,
        }
    }

    pub fn with_coordination_key(mut self, coordination_key: impl Into<String>) -> Self {
        self.coordination_key = Some(coordination_key.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub job_id: String,
    pub worker_id: String,
    pub role: String,
    pub repo: String,
}

#[derive(Debug, Default)]
pub struct DispatchCoordinator {
    registry: WorkerRegistry,
    pending: VecDeque<WorkItem>,
    assigned: BTreeMap<String, (String, WorkItem)>,
    /// Capacity held while the durable Forge claim is being written. Reserved
    /// jobs are deliberately absent from `assigned` and worker in-flight
    /// snapshots until [`commit_reservation`](Self::commit_reservation).
    reserved: BTreeMap<String, (String, WorkItem)>,
    /// Configured finite limits by role. A missing role is unlimited; zero
    /// deliberately prevents every assignment for that role.
    role_limits: BTreeMap<String, u32>,
}

impl DispatchCoordinator {
    /// Construct a coordinator with no finite per-role limits.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a coordinator with authoritative finite per-role limits.
    pub fn with_role_limits(role_limits: BTreeMap<String, u32>) -> Self {
        Self {
            role_limits,
            ..Self::default()
        }
    }

    /// All configured finite role limits. Roles absent from this map are
    /// unlimited.
    pub fn configured_role_limits(&self) -> &BTreeMap<String, u32> {
        &self.role_limits
    }

    /// The configured finite limit for `role`, or `None` when it is unlimited.
    pub fn configured_role_limit(&self, role: &str) -> Option<u32> {
        self.role_limits.get(role).copied()
    }

    pub fn registry(&self) -> &WorkerRegistry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut WorkerRegistry {
        &mut self.registry
    }

    pub fn register(&mut self, msg: &Register) {
        self.registry.register(msg);
    }

    pub fn enqueue(&mut self, item: WorkItem) {
        if self.assigned.contains_key(&item.job_id)
            || self.reserved.contains_key(&item.job_id)
            || self
                .pending
                .iter()
                .any(|pending| pending.job_id == item.job_id)
        {
            return;
        }

        self.pending.push_back(item);
    }

    pub fn dispatch_next(&mut self) -> Option<Assignment> {
        let reservation = self.reserve_next()?;
        self.commit_reservation(&reservation.job_id).ok()
    }

    /// Reserve the next eligible item without publishing it as in-flight.
    pub fn reserve_next(&mut self) -> Option<Assignment> {
        let (index, worker_id) = self.pending.iter().enumerate().find_map(|(index, item)| {
            if !self.candidate_is_eligible(item, None) {
                return None;
            }
            self.registry
                .worker_ids()
                .filter(|worker_id| self.candidate_is_eligible(item, Some(worker_id)))
                .filter_map(|worker_id| {
                    let reserved = self
                        .reserved
                        .values()
                        .filter(|(reserved_worker, _)| reserved_worker == worker_id)
                        .count() as u32;
                    let capacity = self
                        .registry
                        .free_capacity(worker_id)?
                        .saturating_sub(reserved);
                    Some((worker_id, capacity))
                })
                .max_by(|(left_id, left_capacity), (right_id, right_capacity)| {
                    left_capacity
                        .cmp(right_capacity)
                        .then_with(|| right_id.cmp(left_id))
                })
                .map(|(worker_id, _capacity)| (index, worker_id.to_string()))
        })?;
        self.reserve_at(index, worker_id)
    }

    /// Pull-model reservation for a specific requesting worker.
    pub fn reserve_for_worker(&mut self, worker_id: &str) -> Option<Assignment> {
        let index = self
            .pending
            .iter()
            .position(|item| self.candidate_is_eligible(item, Some(worker_id)))?;
        self.reserve_at(index, worker_id.to_string())
    }

    fn reserve_at(&mut self, index: usize, worker_id: String) -> Option<Assignment> {
        let item = self.pending.remove(index)?;
        let reservation = Assignment {
            job_id: item.job_id.clone(),
            worker_id: worker_id.clone(),
            role: item.role.clone(),
            repo: item.repo.clone(),
        };
        self.reserved.insert(item.job_id.clone(), (worker_id, item));
        Some(reservation)
    }

    /// Publish a reservation as in-flight after its durable claim succeeds.
    pub fn commit_reservation(&mut self, job_id: &str) -> Result<Assignment, RegistryError> {
        let Some((worker_id, item)) = self.reserved.remove(job_id) else {
            return Err(RegistryError::DuplicateJob(job_id.to_string()));
        };
        if let Err(error) = self.registry.record_assignment(&worker_id, job_id) {
            self.pending.push_front(item);
            return Err(error);
        }
        let assignment = Assignment {
            job_id: item.job_id.clone(),
            worker_id: worker_id.clone(),
            role: item.role.clone(),
            repo: item.repo.clone(),
        };
        self.assigned.insert(item.job_id.clone(), (worker_id, item));
        Ok(assignment)
    }

    /// Cancel a reservation and restore its original work item to the queue.
    pub fn rollback_reservation(&mut self, job_id: &str) -> bool {
        let Some((_worker_id, item)) = self.reserved.remove(job_id) else {
            return false;
        };
        self.pending.push_front(item);
        true
    }

    /// Pull-model placement retained as a reserve+commit convenience.
    pub fn dispatch_for_worker(&mut self, worker_id: &str) -> Option<Assignment> {
        let reservation = self.reserve_for_worker(worker_id)?;
        self.commit_reservation(&reservation.job_id).ok()
    }

    /// Retract a committed assignment whose response could not be delivered.
    pub fn rollback_committed(&mut self, job_id: &str) -> bool {
        let Some((worker_id, item)) = self.assigned.remove(job_id) else {
            return false;
        };
        let _ = self.registry.complete_job(&worker_id, job_id);
        self.pending.push_front(item);
        true
    }

    /// Reconstitutes an assignment discovered in durable Forge metadata.
    ///
    /// Unlike normal dispatch this never selects another worker: the recorded
    /// worker must be registered, capable, healthy, and have capacity. Global
    /// role concurrency and `(role, coordination_key)` exclusion are enforced
    /// before any in-memory state is changed. Repeating the same restoration is
    /// an idempotent success; every other duplicate is rejected.
    pub fn restore_assignment(
        &mut self,
        worker_id: &str,
        item: WorkItem,
    ) -> Result<Assignment, RegistryError> {
        if let Some((assigned_worker, assigned_item)) = self.assigned.get(&item.job_id) {
            if assigned_worker == worker_id && assigned_item == &item {
                return Ok(Assignment {
                    job_id: item.job_id,
                    worker_id: worker_id.to_string(),
                    role: item.role,
                    repo: item.repo,
                });
            }
            return Err(RegistryError::DuplicateJob(item.job_id));
        }
        if self.reserved.contains_key(&item.job_id)
            || self
                .pending
                .iter()
                .any(|pending| pending.job_id == item.job_id)
        {
            return Err(RegistryError::DuplicateJob(item.job_id));
        }
        if self.role_limit_reached(&item.role) {
            return Err(RegistryError::RoleCapacity(item.role));
        }
        if self.in_flight_workstream_conflicts(&item) {
            return Err(RegistryError::WorkstreamConflict(item.job_id));
        }
        if !self
            .registry
            .can_handle_all(worker_id, &item.role, &item.repos)
        {
            return Err(RegistryError::IneligibleWorker(worker_id.to_string()));
        }
        self.registry.restore_assignment(worker_id, &item.job_id)?;

        let assignment = Assignment {
            job_id: item.job_id.clone(),
            worker_id: worker_id.to_string(),
            role: item.role.clone(),
            repo: item.repo.clone(),
        };
        self.assigned
            .insert(item.job_id.clone(), (worker_id.to_string(), item));
        Ok(assignment)
    }

    pub fn dispatch_ready(&mut self) -> Vec<Assignment> {
        let mut assignments = Vec::new();
        while let Some(assignment) = self.dispatch_next() {
            assignments.push(assignment);
        }
        assignments
    }

    pub fn complete(&mut self, job_id: &str) -> Result<(), RegistryError> {
        let Some((worker_id, _item)) = self.assigned.remove(job_id) else {
            return Ok(());
        };

        self.registry.complete_job(&worker_id, job_id)
    }

    pub fn reclaim_worker(&mut self, worker_id: &str) -> Vec<String> {
        let reclaimed = self.registry.mark_unhealthy(worker_id);
        let mut items = Vec::new();

        for job_id in &reclaimed {
            if let Some((_worker_id, item)) = self.assigned.remove(job_id) {
                items.push(item);
            }
        }
        let reserved = self
            .reserved
            .iter()
            .filter(|(_job_id, (reserved_worker, _))| reserved_worker == worker_id)
            .map(|(job_id, _)| job_id.clone())
            .collect::<Vec<_>>();
        for job_id in reserved {
            if let Some((_worker_id, item)) = self.reserved.remove(&job_id) {
                items.push(item);
            }
        }

        for item in items.into_iter().rev() {
            self.pending.push_front(item);
        }

        reclaimed
    }

    pub fn reserved_len(&self) -> usize {
        self.reserved.len()
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn pending(&self) -> &VecDeque<WorkItem> {
        &self.pending
    }

    /// Retain only `current_job_ids` among pending jobs in the `(repo, role)`
    /// scope, returning the stale pending items that were removed.
    ///
    /// Assigned/in-flight jobs are intentionally not inspected or modified:
    /// this reconciliation seam is only for daemon queue entries that have not
    /// yet been handed to a worker.
    pub fn retain_pending_by_scope(
        &mut self,
        repo: &str,
        role: &str,
        current_job_ids: &BTreeSet<String>,
    ) -> Vec<WorkItem> {
        let mut retained = VecDeque::with_capacity(self.pending.len());
        let mut removed = Vec::new();

        while let Some(item) = self.pending.pop_front() {
            if item.repo == repo && item.role == role && !current_job_ids.contains(&item.job_id) {
                removed.push(item);
            } else {
                retained.push_back(item);
            }
        }

        self.pending = retained;
        removed
    }

    pub fn assigned_worker(&self, job_id: &str) -> Option<&str> {
        self.assigned
            .get(job_id)
            .map(|(worker_id, _item)| worker_id.as_str())
    }

    /// The in-flight work item for `job_id`, if currently assigned.
    pub fn assigned_work_item(&self, job_id: &str) -> Option<&WorkItem> {
        self.assigned.get(job_id).map(|(_worker_id, item)| item)
    }

    /// All currently in-flight (assigned, not yet completed) work items.
    ///
    /// Used by concurrency observability to tell whether a role already holds a
    /// worker slot (and so further same-role pending work is queued behind it).
    pub fn assigned_work_items(&self) -> impl Iterator<Item = &WorkItem> {
        self.assigned.values().map(|(_worker_id, item)| item)
    }

    pub fn in_flight_len(&self) -> usize {
        self.assigned.len()
    }

    /// Shared eligibility predicate for both push and pull dispatch. Global
    /// role/workstream constraints are always checked; when a worker is known,
    /// its health, free capacity, role capability, and repository capabilities
    /// are checked independently as well.
    fn candidate_is_eligible(&self, item: &WorkItem, worker_id: Option<&str>) -> bool {
        if self.role_limit_reached(&item.role) || self.in_flight_workstream_conflicts(item) {
            return false;
        }

        let Some(worker_id) = worker_id else {
            return true;
        };
        let reserved_for_worker = self
            .reserved
            .values()
            .filter(|(reserved_worker, _)| reserved_worker == worker_id)
            .count() as u32;
        matches!(
            self.registry.free_capacity(worker_id),
            Some(capacity) if capacity > reserved_for_worker
        ) && self.registry.is_healthy(worker_id)
            && self
                .registry
                .can_handle_all(worker_id, &item.role, &item.repos)
    }

    fn role_limit_reached(&self, role: &str) -> bool {
        self.configured_role_limit(role).is_some_and(|limit| {
            self.assigned
                .values()
                .chain(self.reserved.values())
                .filter(|(_worker_id, item)| item.role == role)
                .count()
                >= limit as usize
        })
    }

    fn in_flight_workstream_conflicts(&self, item: &WorkItem) -> bool {
        let Some(coordination_key) = item
            .coordination_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
        else {
            return false;
        };

        self.assigned
            .values()
            .chain(self.reserved.values())
            .any(|(_worker_id, assigned)| {
                assigned.role == item.role
                    && assigned
                        .coordination_key
                        .as_deref()
                        .map(str::trim)
                        .is_some_and(|key| key == coordination_key)
            })
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod dispatch_tests;
