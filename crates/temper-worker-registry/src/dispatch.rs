// SPDX-License-Identifier: MPL-2.0

//! Pure in-memory dispatch coordination over the soft worker registry.

use std::collections::{BTreeMap, VecDeque};

use temper_worker_protocol::Register;

use crate::{RegistryError, WorkerRegistry};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkItem {
    pub job_id: String,
    pub role: String,
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
            repos: vec![repo.clone()],
            repo,
        }
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
}

impl DispatchCoordinator {
    pub fn new() -> Self {
        Self::default()
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
        let (index, worker_id) = self.pending.iter().enumerate().find_map(|(index, item)| {
            self.registry
                .assign_candidate_all(&item.role, &item.repos)
                .map(|worker_id| (index, worker_id))
        })?;

        let item = self
            .pending
            .remove(index)
            .expect("pending index came from iterating pending queue");

        if self
            .registry
            .record_assignment(&worker_id, &item.job_id)
            .is_err()
        {
            self.pending.push_front(item);
            return None;
        }

        let assignment = Assignment {
            job_id: item.job_id.clone(),
            worker_id: worker_id.clone(),
            role: item.role.clone(),
            repo: item.repo.clone(),
        };
        self.assigned.insert(item.job_id.clone(), (worker_id, item));

        Some(assignment)
    }

    /// Pull-model placement for a specific requesting worker.
    pub fn dispatch_for_worker(&mut self, worker_id: &str) -> Option<Assignment> {
        match self.registry.free_capacity(worker_id) {
            Some(free_capacity) if free_capacity > 0 && self.registry.is_healthy(worker_id) => {}
            _ => return None,
        }

        let index = self.pending.iter().position(|item| {
            self.registry
                .can_handle_all(worker_id, &item.role, &item.repos)
        })?;

        let item = self
            .pending
            .remove(index)
            .expect("pending index came from iterating pending queue");

        if self
            .registry
            .record_assignment(worker_id, &item.job_id)
            .is_err()
        {
            self.pending.push_front(item);
            return None;
        }

        let assignment = Assignment {
            job_id: item.job_id.clone(),
            worker_id: worker_id.to_string(),
            role: item.role.clone(),
            repo: item.repo.clone(),
        };
        self.assigned
            .insert(item.job_id.clone(), (worker_id.to_string(), item));

        Some(assignment)
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

        for item in items.into_iter().rev() {
            self.pending.push_front(item);
        }

        reclaimed
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn pending(&self) -> &VecDeque<WorkItem> {
        &self.pending
    }

    /// The in-flight work item for `job_id`, if currently assigned.
    pub fn assigned_work_item(&self, job_id: &str) -> Option<&WorkItem> {
        self.assigned.get(job_id).map(|(_worker_id, item)| item)
    }

    pub fn in_flight_len(&self) -> usize {
        self.assigned.len()
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod dispatch_tests;
