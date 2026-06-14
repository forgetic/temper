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
mod tests {
    use super::*;
    use temper_worker_protocol::{Capability, Capacity, WORKER_PROTOCOL_VERSION};

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

    fn work(job_id: &str, role: &str, repo: &str) -> WorkItem {
        WorkItem::single(job_id, role, repo)
    }

    fn register_multi(
        worker_id: &str,
        role: &str,
        repos: &[&str],
        max_concurrent_jobs: u32,
    ) -> Register {
        Register {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.to_string(),
            capabilities: repos
                .iter()
                .map(|repo| Capability {
                    role: role.to_string(),
                    repo: (*repo).to_string(),
                })
                .collect(),
            capacity: Capacity {
                max_concurrent_jobs,
            },
            labels: None,
        }
    }

    fn coordinated(job_id: &str, role: &str, repos: &[&str]) -> WorkItem {
        WorkItem {
            job_id: job_id.to_string(),
            role: role.to_string(),
            repo: repos[0].to_string(),
            repos: repos.iter().map(|repo| (*repo).to_string()).collect(),
        }
    }

    #[test]
    fn enqueue_then_dispatch_assigns_to_capable_worker() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
        coordinator.enqueue(work("job-1", "engineer", "ai/temper"));

        assert_eq!(
            coordinator.dispatch_next(),
            Some(Assignment {
                job_id: "job-1".to_string(),
                worker_id: "worker-a".to_string(),
                role: "engineer".to_string(),
                repo: "ai/temper".to_string(),
            })
        );
        assert_eq!(coordinator.pending_len(), 0);
        assert_eq!(coordinator.in_flight_len(), 1);
    }

    #[test]
    fn dispatch_for_worker_assigns_a_matching_item_to_the_requesting_worker() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
        coordinator.register(&register("worker-b", "engineer", "ai/temper", 1));
        coordinator.enqueue(work("job-1", "engineer", "ai/temper"));

        assert_eq!(
            coordinator.dispatch_for_worker("worker-a"),
            Some(Assignment {
                job_id: "job-1".to_string(),
                worker_id: "worker-a".to_string(),
                role: "engineer".to_string(),
                repo: "ai/temper".to_string(),
            })
        );
    }

    #[test]
    fn coordinated_job_only_dispatches_to_a_worker_capable_of_all_repos() {
        let mut coordinator = DispatchCoordinator::new();
        // Capable of the primary + smith, but NOT skein.
        coordinator.register(&register_multi(
            "worker-partial",
            "engineer",
            &["ai/temper", "ai/smith"],
            2,
        ));
        coordinator.enqueue(coordinated(
            "job-coord",
            "engineer",
            &["ai/temper", "ai/smith", "ai/skein"],
        ));

        // No worker covers all three repos: the job stays pending.
        assert_eq!(coordinator.dispatch_for_worker("worker-partial"), None);
        assert_eq!(coordinator.dispatch_next(), None);
        assert_eq!(coordinator.pending_len(), 1);

        // A worker capable of every manifest repo picks it up; the assignment
        // carries the primary repo.
        coordinator.register(&register_multi(
            "worker-full",
            "engineer",
            &["ai/temper", "ai/smith", "ai/skein"],
            2,
        ));
        let assignment = coordinator.dispatch_for_worker("worker-full").unwrap();
        assert_eq!(assignment.job_id, "job-coord");
        assert_eq!(assignment.worker_id, "worker-full");
        assert_eq!(assignment.repo, "ai/temper");
    }

    #[test]
    fn dispatch_for_worker_skips_items_the_worker_cannot_handle() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
        coordinator.enqueue(work("job-1", "architect", "ai/temper"));
        coordinator.enqueue(work("job-2", "engineer", "ai/temper"));

        assert_eq!(
            coordinator.dispatch_for_worker("worker-a").unwrap().job_id,
            "job-2"
        );
        assert_eq!(coordinator.pending_len(), 1);
        assert_eq!(coordinator.dispatch_next(), None);
    }

    #[test]
    fn dispatch_for_worker_returns_none_when_saturated() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
        coordinator
            .registry_mut()
            .record_assignment("worker-a", "job-in-flight")
            .unwrap();
        coordinator.enqueue(work("job-1", "engineer", "ai/temper"));

        assert_eq!(coordinator.dispatch_for_worker("worker-a"), None);
        assert_eq!(coordinator.pending_len(), 1);
    }

    #[test]
    fn dispatch_for_worker_returns_none_for_unknown_or_unhealthy_worker() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.enqueue(work("job-1", "engineer", "ai/temper"));
        assert_eq!(coordinator.dispatch_for_worker("missing"), None);

        coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
        coordinator.registry_mut().mark_unhealthy("worker-a");
        assert_eq!(coordinator.dispatch_for_worker("worker-a"), None);
        assert_eq!(coordinator.pending_len(), 1);
    }

    #[test]
    fn dispatch_for_worker_is_fifo_among_handleable_items() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("worker-a", "engineer", "ai/temper", 3));
        coordinator.enqueue(work("job-1", "architect", "ai/temper"));
        coordinator.enqueue(work("job-2", "engineer", "ai/temper"));
        coordinator.enqueue(work("job-3", "reviewer", "ai/temper"));
        coordinator.enqueue(work("job-4", "engineer", "ai/temper"));

        assert_eq!(
            coordinator.dispatch_for_worker("worker-a").unwrap().job_id,
            "job-2"
        );
        assert_eq!(
            coordinator.dispatch_for_worker("worker-a").unwrap().job_id,
            "job-4"
        );
        assert_eq!(coordinator.pending_len(), 2);
    }

    #[test]
    fn two_workers_each_only_receive_their_capability_items() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("engineer-worker", "engineer", "ai/temper", 1));
        coordinator.register(&register("architect-worker", "architect", "ai/temper", 1));
        coordinator.enqueue(work("job-engineer", "engineer", "ai/temper"));
        coordinator.enqueue(work("job-architect", "architect", "ai/temper"));

        let engineer = coordinator.dispatch_for_worker("engineer-worker").unwrap();
        assert_eq!(engineer.job_id, "job-engineer");
        assert_eq!(engineer.worker_id, "engineer-worker");

        let architect = coordinator.dispatch_for_worker("architect-worker").unwrap();
        assert_eq!(architect.job_id, "job-architect");
        assert_eq!(architect.worker_id, "architect-worker");
    }

    #[test]
    fn no_capable_worker_defers_item() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
        coordinator.enqueue(work("job-1", "reviewer", "ai/temper"));

        assert_eq!(coordinator.dispatch_ready(), Vec::new());
        assert_eq!(coordinator.pending_len(), 1);
        assert_eq!(coordinator.in_flight_len(), 0);
    }

    #[test]
    fn saturated_then_complete_allows_dispatch() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
        coordinator.enqueue(work("job-1", "engineer", "ai/temper"));
        coordinator.enqueue(work("job-2", "engineer", "ai/temper"));

        assert_eq!(coordinator.dispatch_next().unwrap().job_id, "job-1");
        assert_eq!(coordinator.dispatch_next(), None);
        assert_eq!(coordinator.pending_len(), 1);

        coordinator.complete("job-1").unwrap();
        assert_eq!(coordinator.dispatch_next().unwrap().job_id, "job-2");
    }

    #[test]
    fn dispatch_ready_places_up_to_capacity_then_defers() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("worker-a", "engineer", "ai/temper", 2));
        for job_id in ["job-1", "job-2", "job-3"] {
            coordinator.enqueue(work(job_id, "engineer", "ai/temper"));
        }

        let assignments = coordinator.dispatch_ready();
        assert_eq!(assignments.len(), 2);
        assert_eq!(coordinator.pending_len(), 1);
        assert_eq!(coordinator.in_flight_len(), 2);
    }

    #[test]
    fn complete_routes_to_the_correct_worker_and_frees_capacity() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
        coordinator.register(&register("worker-b", "engineer", "ai/temper", 2));
        coordinator.enqueue(work("job-1", "engineer", "ai/temper"));
        coordinator.enqueue(work("job-2", "engineer", "ai/temper"));
        coordinator.enqueue(work("job-3", "engineer", "ai/temper"));

        let first = coordinator.dispatch_next().unwrap();
        assert_eq!(first.worker_id, "worker-b");
        let second = coordinator.dispatch_next().unwrap();
        assert_eq!(second.worker_id, "worker-a");
        assert_eq!(coordinator.dispatch_next().unwrap().worker_id, "worker-b");

        coordinator.enqueue(work("job-4", "engineer", "ai/temper"));
        assert_eq!(coordinator.dispatch_next(), None);
        coordinator.complete(&second.job_id).unwrap();
        let fourth = coordinator.dispatch_next().unwrap();
        assert_eq!(fourth.job_id, "job-4");
        assert_eq!(fourth.worker_id, second.worker_id);
    }

    #[test]
    fn reclaim_worker_requeues_in_flight_jobs_for_reassignment() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
        coordinator.register(&register("worker-b", "engineer", "ai/temper", 1));
        coordinator.registry_mut().mark_unhealthy("worker-a");
        coordinator.enqueue(work("job-1", "engineer", "ai/temper"));

        let assignment = coordinator.dispatch_next().unwrap();
        assert_eq!(assignment.worker_id, "worker-b");
        let reclaimed = coordinator.reclaim_worker("worker-b");
        assert_eq!(reclaimed, vec!["job-1".to_string()]);
        assert_eq!(coordinator.pending_len(), 1);
        assert_eq!(coordinator.in_flight_len(), 0);

        coordinator.registry_mut().heartbeat("worker-a").unwrap();
        let reassignment = coordinator.dispatch_next().unwrap();
        assert_eq!(reassignment.job_id, "job-1");
        assert_eq!(reassignment.worker_id, "worker-a");
    }

    #[test]
    fn enqueue_is_idempotent_for_a_known_job_id() {
        let mut coordinator = DispatchCoordinator::new();
        coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
        coordinator.enqueue(work("job-1", "engineer", "ai/temper"));
        coordinator.enqueue(work("job-1", "reviewer", "ai/smith"));
        assert_eq!(coordinator.pending_len(), 1);

        coordinator.dispatch_next().unwrap();
        coordinator.enqueue(work("job-1", "engineer", "ai/temper"));
        assert_eq!(coordinator.pending_len(), 0);
        assert_eq!(coordinator.in_flight_len(), 1);
    }

    #[test]
    fn dispatch_is_deterministic() {
        fn build_and_dispatch() -> Vec<Assignment> {
            let mut coordinator = DispatchCoordinator::new();
            coordinator.register(&register("worker-b", "engineer", "ai/temper", 2));
            coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
            coordinator.enqueue(work("job-1", "engineer", "ai/temper"));
            coordinator.enqueue(work("job-2", "reviewer", "ai/temper"));
            coordinator.enqueue(work("job-3", "engineer", "ai/temper"));
            coordinator.dispatch_ready()
        }

        let first = build_and_dispatch();
        let second = build_and_dispatch();
        assert_eq!(first, second);
        assert_eq!(
            first
                .iter()
                .map(|assignment| assignment.worker_id.as_str())
                .collect::<Vec<_>>(),
            vec!["worker-b", "worker-a"]
        );
        assert_eq!(
            first
                .iter()
                .map(|assignment| assignment.job_id.as_str())
                .collect::<Vec<_>>(),
            vec!["job-1", "job-3"]
        );
    }
}
