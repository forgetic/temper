// SPDX-License-Identifier: MPL-2.0

use crate::test_support::{register, register_multi};
use crate::{
    RegistrationError, RegistryError, WorkerPoolPolicies, WorkerPoolPolicy, WorkerRegistry,
};

fn builders_policy() -> WorkerPoolPolicies {
    WorkerPoolPolicies::from(vec![WorkerPoolPolicy::new(
        "builders",
        vec!["engineer".to_string(), "reviewer".to_string()],
        vec!["ai/temper".to_string(), "acme/widgets".to_string()],
        Some(2),
    )])
}

#[test]
fn register_with_pool_policy_records_pool_capacity_and_capabilities() {
    let mut registry = WorkerRegistry::new();
    let mut msg = register("worker-a", "engineer", "ai/temper", 2);
    msg.worker_pool = Some("builders".to_string());

    registry
        .register_with_policies(&msg, &builders_policy())
        .expect("pool registration accepted");

    assert_eq!(
        registry.assign_candidate("engineer", "ai/temper"),
        Some("worker-a".to_string())
    );
    let workers = registry.worker_snapshots();
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].worker_id, "worker-a");
    assert_eq!(workers[0].worker_pool.as_deref(), Some("builders"));
    assert_eq!(workers[0].max_concurrent_jobs, 2);
    assert_eq!(workers[0].free_capacity, 2);
    assert_eq!(workers[0].capabilities.len(), 1);
}

#[test]
fn pool_policy_rejects_unknown_pool_capacity_and_capability_mismatches() {
    let policies = builders_policy();
    let mut registry = WorkerRegistry::new();

    let mut unknown = register("worker-a", "engineer", "ai/temper", 1);
    unknown.worker_pool = Some("missing".to_string());
    assert_eq!(
        registry.register_with_policies(&unknown, &policies),
        Err(RegistrationError::UnknownPool("missing".to_string()))
    );

    let mut too_large = register("worker-a", "engineer", "ai/temper", 3);
    too_large.worker_pool = Some("builders".to_string());
    assert_eq!(
        registry.register_with_policies(&too_large, &policies),
        Err(RegistrationError::CapacityExceeded {
            pool: "builders".to_string(),
            requested: 3,
            max: 2,
        })
    );

    let mut wrong_capability = register("worker-a", "architect", "ai/temper", 1);
    wrong_capability.worker_pool = Some("builders".to_string());
    assert_eq!(
        registry.register_with_policies(&wrong_capability, &policies),
        Err(RegistrationError::CapabilityOutsidePool {
            pool: "builders".to_string(),
            role: "architect".to_string(),
            repo: "ai/temper".to_string(),
        })
    );

    assert_eq!(registry.worker_count(), 0);
}

#[test]
fn pool_policy_rejects_missing_capacity_and_empty_worker_identity() {
    let policies = WorkerPoolPolicies::from(vec![WorkerPoolPolicy::new(
        "capacityless",
        vec!["engineer".to_string()],
        vec!["ai/temper".to_string()],
        None,
    )]);
    let mut registry = WorkerRegistry::new();

    let mut msg = register("worker-a", "engineer", "ai/temper", 1);
    msg.worker_pool = Some("capacityless".to_string());
    assert_eq!(
        registry.register_with_policies(&msg, &policies),
        Err(RegistrationError::PoolMissingCapacity(
            "capacityless".to_string()
        ))
    );

    let empty_id = register(" ", "engineer", "ai/temper", 1);
    assert_eq!(
        registry.register_with_policies(&empty_id, &WorkerPoolPolicies::default()),
        Err(RegistrationError::EmptyWorkerId)
    );
}

#[test]
fn no_pool_registration_preserves_legacy_capabilities_even_with_policies() {
    let mut registry = WorkerRegistry::new();
    let msg = register("worker-a", "legacy", "legacy/repo", 1);

    registry
        .register_with_policies(&msg, &builders_policy())
        .expect("legacy no-pool registration remains allowed");

    assert_eq!(
        registry.assign_candidate("legacy", "legacy/repo"),
        Some("worker-a".to_string())
    );
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
fn assign_candidate_all_requires_every_repo_in_the_manifest() {
    let repos = vec![
        "ai/temper".to_string(),
        "ai/smith".to_string(),
        "ai/skein".to_string(),
    ];

    let mut partial = WorkerRegistry::new();
    partial.register(&register_multi(
        "worker-a",
        "engineer",
        &["ai/temper", "ai/smith"],
        1,
    ));
    assert_eq!(partial.assign_candidate_all("engineer", &repos), None);
    assert!(!partial.can_handle_all("worker-a", "engineer", &repos));

    let mut full = WorkerRegistry::new();
    full.register(&register_multi(
        "worker-a",
        "engineer",
        &["ai/temper", "ai/smith", "ai/skein"],
        1,
    ));
    assert_eq!(
        full.assign_candidate_all("engineer", &repos),
        Some("worker-a".to_string())
    );
    assert!(full.can_handle_all("worker-a", "engineer", &repos));
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
fn restore_assignment_is_idempotent_but_never_exceeds_capacity() {
    let mut registry = WorkerRegistry::new();
    registry.register(&register("worker-a", "engineer", "ai/temper", 1));

    registry
        .restore_assignment("worker-a", "job-1")
        .expect("first restoration succeeds");
    registry
        .restore_assignment("worker-a", "job-1")
        .expect("repeated restoration is idempotent");
    assert_eq!(registry.free_capacity("worker-a"), Some(0));
    assert_eq!(
        registry.restore_assignment("worker-a", "job-2"),
        Err(RegistryError::NoCapacity("worker-a".to_string()))
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
