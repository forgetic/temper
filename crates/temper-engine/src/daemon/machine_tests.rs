// SPDX-License-Identifier: MPL-2.0

use super::*;
use serde_json::json;
use temper_forge::{ChangeKind, HintArtifactKind, ItemNumber, RepositoryPath};
use temper_protocol_worker::Artifact;
use temper_workflow::RoleId;

#[test]
fn nested_applies_release_one_deferred_repository_generation_only_after_final_completion() {
    let mut machine = DaemonMachine::default_machine(Duration::ZERO);
    let repository = RepositoryPath::new("ai", "temper");
    let lane = WakeLane::Role(RoleId::new("engineer"));
    machine
        .wake_coordinator
        .configure_repository(repository.clone(), [lane.clone()]);
    machine
        .applying
        .extend(["apply-a".to_string(), "apply-b".to_string()]);

    for index in 0..100 {
        let requests = machine.on_completion(
            EngineTime::from_nanos(index),
            DaemonCompletion::ScheduleWake {
                request: WakeRequest::targeted_for_lanes(
                    repository.clone(),
                    [lane.clone()],
                    HintArtifactKind::Issue,
                    ItemNumber::new((index % 4) + 1),
                    ChangeKind::Label,
                ),
            },
        );
        assert!(requests.is_empty(), "no timer or run starts during apply");
    }

    let nested = machine.on_completion(
        EngineTime::from_nanos(101),
        DaemonCompletion::ApplyFinished {
            job_id: "apply-a".to_string(),
            outcome: ApplyOutcome::Applied,
        },
    );
    assert!(
        nested.is_empty(),
        "nested completion does not promote wakes"
    );
    let final_requests = machine.on_completion(
        EngineTime::from_nanos(102),
        DaemonCompletion::ApplyFinished {
            job_id: "apply-b".to_string(),
            outcome: ApplyOutcome::Applied,
        },
    );
    assert_eq!(
        final_requests
            .iter()
            .filter(|request| matches!(request, DaemonRequest::StartWakeTimer { .. }))
            .count(),
        1
    );
    assert!(
        !final_requests
            .iter()
            .any(|request| matches!(request, DaemonRequest::RunWake { .. }))
    );
}

#[test]
fn startup_recovery_barrier_defers_enqueue_until_orphans_are_collected() {
    let mut machine = DaemonMachine::default_machine(Duration::ZERO);
    machine.on_completion(EngineTime::ZERO, DaemonCompletion::BeginStartupRecovery);
    let requests = machine.on_completion(
        EngineTime::ZERO,
        DaemonCompletion::Enqueue {
            job_id: "job-after-recovery".to_string(),
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            artifact: Artifact {
                item: json!(258),
                kind: "issue".to_string(),
            },
            job_payload: json!({}),
        },
    );
    assert!(requests.is_empty());
    assert!(machine.core.queued_jobs().is_empty());
    assert_eq!(machine.deferred_enqueues.len(), 1);

    let (reply, _rx) = temper_engine_io::oneshot();
    machine.on_completion(
        EngineTime::ZERO,
        DaemonCompletion::CollectStartupOrphans { reply },
    );
    assert!(machine.startup_recovery);
    assert_eq!(machine.deferred_enqueues.len(), 1);
    assert!(machine.core.queued_jobs().is_empty());

    let (reply, _rx) = temper_engine_io::oneshot();
    machine.on_completion(
        EngineTime::ZERO,
        DaemonCompletion::CompleteStartupRecovery { reply },
    );
    assert!(!machine.startup_recovery);
    assert!(machine.deferred_enqueues.is_empty());
    assert_eq!(machine.core.queued_jobs().len(), 1);
}

#[test]
fn retryable_apply_uses_observable_bounded_exponential_backoff() {
    assert_eq!(retry_delay(1), Duration::from_secs(1));
    assert_eq!(retry_delay(2), Duration::from_secs(2));
    assert_eq!(retry_delay(20), Duration::from_secs(256));

    let mut machine = DaemonMachine::default_machine(Duration::ZERO);
    let requests = machine.on_completion(
        EngineTime::ZERO,
        DaemonCompletion::ApplyFinished {
            job_id: "job-1".to_string(),
            outcome: ApplyOutcome::Retryable {
                reason: "temporary Forge outage".to_string(),
            },
        },
    );
    assert_eq!(machine.retry_attempts.get("job-1"), Some(&1));
    assert!(requests.iter().any(|request| matches!(
        request,
        DaemonRequest::Log(line)
            if line.contains("attempt=1")
                && line.contains("backoff_ms=1000")
                && line.contains("temporary Forge outage")
    )));

    let requests = machine.on_completion(
        EngineTime::ZERO,
        DaemonCompletion::Enqueue {
            job_id: "job-1".to_string(),
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            artifact: Artifact {
                item: json!(1),
                kind: "issue".to_string(),
            },
            job_payload: json!({}),
        },
    );
    assert!(requests.iter().any(|request| matches!(
        request,
        DaemonRequest::Log(line) if line.contains("retry backoff")
    )));
    assert!(machine.core.queued_jobs().is_empty());
}
