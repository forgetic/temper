// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, BTreeSet};

use temper_protocol_worker::Capability;

use crate::test_support::{coordinated, coordinated_workstream, register, register_multi, work};
use crate::{Assignment, DispatchCoordinator};

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

    assert_eq!(coordinator.dispatch_for_worker("worker-partial"), None);
    assert_eq!(coordinator.dispatch_next(), None);
    assert_eq!(coordinator.pending_len(), 1);

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
fn same_role_same_workstream_is_not_dispatched_concurrently() {
    let mut coordinator = DispatchCoordinator::new();
    coordinator.register(&register("worker-a", "engineer", "ai/temper", 3));
    coordinator.enqueue(coordinated_workstream(
        "source-issue-job",
        "engineer",
        "ai/temper",
        "pr-for-code-463",
    ));
    coordinator.enqueue(coordinated_workstream(
        "pr-repair-job",
        "engineer",
        "ai/temper",
        "pr-for-code-463",
    ));
    coordinator.enqueue(coordinated_workstream(
        "unrelated-job",
        "engineer",
        "ai/temper",
        "pr-for-code-999",
    ));

    assert_eq!(
        coordinator.dispatch_for_worker("worker-a").unwrap().job_id,
        "source-issue-job"
    );
    assert_eq!(
        coordinator.dispatch_for_worker("worker-a").unwrap().job_id,
        "unrelated-job"
    );
    assert_eq!(coordinator.dispatch_for_worker("worker-a"), None);
    assert_eq!(
        coordinator
            .pending()
            .iter()
            .map(|item| item.job_id.as_str())
            .collect::<Vec<_>>(),
        vec!["pr-repair-job"]
    );
    assert_eq!(coordinator.in_flight_len(), 2);
}

#[test]
fn completed_paused_workstream_does_not_suppress_later_same_key_job() {
    let mut coordinator = DispatchCoordinator::new();
    coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
    coordinator.enqueue(coordinated_workstream(
        "source-issue-job",
        "engineer",
        "ai/temper",
        "pr-for-code-483",
    ));

    assert_eq!(
        coordinator.dispatch_for_worker("worker-a").unwrap().job_id,
        "source-issue-job"
    );
    coordinator.complete("source-issue-job").unwrap();
    assert_eq!(coordinator.in_flight_len(), 0);

    coordinator.enqueue(coordinated_workstream(
        "pr-feedback-job",
        "engineer",
        "ai/temper",
        "pr-for-code-483",
    ));
    assert_eq!(
        coordinator.dispatch_for_worker("worker-a").unwrap().job_id,
        "pr-feedback-job"
    );
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
fn scoped_pending_retain_prunes_only_matching_pending_jobs() {
    let mut coordinator = DispatchCoordinator::new();
    coordinator.register(&register("worker-a", "engineer", "ai/temper", 1));
    coordinator.enqueue(work("assigned", "engineer", "ai/temper"));
    assert_eq!(coordinator.dispatch_next().unwrap().job_id, "assigned");

    coordinator.enqueue(work("stale", "engineer", "ai/temper"));
    coordinator.enqueue(work("current", "engineer", "ai/temper"));
    coordinator.enqueue(work("other-role", "architect", "ai/temper"));
    coordinator.enqueue(work("other-repo", "engineer", "ai/other"));

    let current = BTreeSet::from(["current".to_string()]);
    let removed = coordinator.retain_pending_by_scope("ai/temper", "engineer", &current);

    assert_eq!(
        removed
            .iter()
            .map(|item| item.job_id.as_str())
            .collect::<Vec<_>>(),
        vec!["stale"]
    );
    assert_eq!(
        coordinator
            .pending()
            .iter()
            .map(|item| item.job_id.as_str())
            .collect::<Vec<_>>(),
        vec!["current", "other-role", "other-repo"]
    );
    assert_eq!(coordinator.in_flight_len(), 1);
    assert!(coordinator.assigned_work_item("assigned").is_some());
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
fn finite_role_limit_one_serializes_distinct_workstreams_despite_worker_capacity() {
    let mut coordinator =
        DispatchCoordinator::with_role_limits(BTreeMap::from([("engineer".to_string(), 1)]));
    assert_eq!(coordinator.configured_role_limit("engineer"), Some(1));
    assert_eq!(coordinator.configured_role_limit("reviewer"), None);
    coordinator.register(&register("worker-a", "engineer", "ai/temper", 4));
    coordinator.enqueue(coordinated_workstream(
        "job-1",
        "engineer",
        "ai/temper",
        "stream-1",
    ));
    coordinator.enqueue(coordinated_workstream(
        "job-2",
        "engineer",
        "ai/temper",
        "stream-2",
    ));

    assert_eq!(coordinator.dispatch_next().unwrap().job_id, "job-1");
    assert_eq!(coordinator.dispatch_next(), None);
    assert_eq!(coordinator.pending_len(), 1);
}

#[test]
fn finite_role_limit_two_assigns_two_and_leaves_third_pending() {
    let mut coordinator =
        DispatchCoordinator::with_role_limits(BTreeMap::from([("engineer".to_string(), 2)]));
    coordinator.register(&register("worker-a", "engineer", "ai/temper", 4));
    for job_id in ["job-1", "job-2", "job-3"] {
        coordinator.enqueue(work(job_id, "engineer", "ai/temper"));
    }

    assert_eq!(coordinator.dispatch_ready().len(), 2);
    assert_eq!(coordinator.pending_len(), 1);
}

#[test]
fn finite_role_limit_is_shared_across_workers_and_repositories() {
    let mut coordinator =
        DispatchCoordinator::with_role_limits(BTreeMap::from([("engineer".to_string(), 1)]));
    coordinator.register(&register_multi(
        "worker-a",
        "engineer",
        &["ai/temper", "ai/smith"],
        2,
    ));
    coordinator.register(&register_multi(
        "worker-b",
        "engineer",
        &["ai/temper", "ai/smith"],
        2,
    ));
    coordinator.enqueue(work("temper-job", "engineer", "ai/temper"));
    coordinator.enqueue(work("smith-job", "engineer", "ai/smith"));

    assert!(coordinator.dispatch_next().is_some());
    assert_eq!(coordinator.dispatch_next(), None);
    assert_eq!(coordinator.pending_len(), 1);
}

#[test]
fn roles_without_limits_use_all_advertised_worker_capacity() {
    let mut coordinator =
        DispatchCoordinator::with_role_limits(BTreeMap::from([("reviewer".to_string(), 1)]));
    coordinator.register(&register("worker-a", "engineer", "ai/temper", 4));
    for job_id in ["job-1", "job-2", "job-3", "job-4", "job-5"] {
        coordinator.enqueue(work(job_id, "engineer", "ai/temper"));
    }

    assert_eq!(coordinator.dispatch_ready().len(), 4);
    assert_eq!(coordinator.pending_len(), 1);
}

#[test]
fn push_and_pull_dispatch_skip_saturated_roles_without_blocking_other_roles() {
    let limits = BTreeMap::from([("engineer".to_string(), 1)]);
    let mut push = DispatchCoordinator::with_role_limits(limits.clone());
    push.register(&register("engineer", "engineer", "ai/temper", 4));
    push.register(&register("architect", "architect", "ai/temper", 1));
    push.enqueue(work("eng-1", "engineer", "ai/temper"));
    push.enqueue(work("eng-2", "engineer", "ai/temper"));
    push.enqueue(work("arch-1", "architect", "ai/temper"));
    assert_eq!(push.dispatch_next().unwrap().job_id, "eng-1");
    assert_eq!(push.dispatch_next().unwrap().job_id, "arch-1");

    let mut pull = DispatchCoordinator::with_role_limits(limits);
    let mut worker = register("worker", "engineer", "ai/temper", 4);
    worker.capabilities.push(Capability {
        role: "architect".to_string(),
        repo: "ai/temper".to_string(),
    });
    pull.register(&worker);
    pull.enqueue(work("eng-1", "engineer", "ai/temper"));
    pull.enqueue(work("eng-2", "engineer", "ai/temper"));
    pull.enqueue(work("arch-1", "architect", "ai/temper"));
    assert_eq!(pull.dispatch_for_worker("worker").unwrap().job_id, "eng-1");
    assert_eq!(pull.dispatch_for_worker("worker").unwrap().job_id, "arch-1");
}

#[test]
fn completion_and_reclamation_reopen_finite_role_capacity() {
    let limits = BTreeMap::from([("engineer".to_string(), 1)]);
    let mut coordinator = DispatchCoordinator::with_role_limits(limits);
    coordinator.register(&register("worker-a", "engineer", "ai/temper", 2));
    coordinator.register(&register("worker-b", "engineer", "ai/temper", 2));
    coordinator.enqueue(work("job-1", "engineer", "ai/temper"));
    coordinator.enqueue(work("job-2", "engineer", "ai/temper"));
    coordinator.enqueue(work("job-3", "engineer", "ai/temper"));

    let first = coordinator.dispatch_next().unwrap();
    coordinator.complete(&first.job_id).unwrap();
    let second = coordinator.dispatch_next().unwrap();
    assert_eq!(second.job_id, "job-2");

    coordinator.reclaim_worker(&second.worker_id);
    let replacement = coordinator.dispatch_next().unwrap();
    assert_eq!(replacement.job_id, "job-2");
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
