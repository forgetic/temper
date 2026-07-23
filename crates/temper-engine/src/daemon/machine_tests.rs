// SPDX-License-Identifier: MPL-2.0

use super::*;
use serde_json::json;
use temper_engine_io::oneshot;
use temper_forge::{ChangeKind, HintArtifactKind, ItemNumber, RepositoryPath};
use temper_protocol_worker::Artifact;
use temper_workflow::RoleId;

#[test]
fn begin_shutdown_closes_dispatch_before_assignment_release() {
    let mut machine = DaemonMachine::default_machine(Duration::ZERO);
    let (reply, _report) = oneshot();
    let (joined, _join_notification) = oneshot();
    let requests = machine.on_completion(
        EngineTime::ZERO,
        DaemonCompletion::BeginShutdown { reply, joined },
    );
    assert!(requests.is_empty());
    assert_eq!(machine.shutdown_admission, ShutdownAdmission::Fenced);
    assert!(machine.core.in_flight_jobs().is_empty());

    let enqueue = machine.on_completion(
        EngineTime::ZERO,
        DaemonCompletion::Enqueue {
            job_id: "late-job".to_string(),
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            artifact: Artifact {
                item: json!(454),
                kind: "issue".to_string(),
            },
            job_payload: json!({}),
        },
    );
    assert!(enqueue.is_empty());
    assert!(machine.core.queued_jobs().is_empty());
}

#[test]
fn shutdown_report_tracks_admitted_context_until_its_typed_guard_finishes() {
    let mut machine = DaemonMachine::default_machine(Duration::ZERO);
    let identity = AssignmentAttemptIdentity::new(
        "worker-context",
        "job-context",
        Some("attempt-context".to_string()),
    );
    let admission = machine.admit_context(identity.clone());
    let (reply, report) = oneshot();
    let (joined, join_notification) = oneshot();

    machine.on_completion(
        EngineTime::ZERO,
        DaemonCompletion::BeginShutdown { reply, joined },
    );
    let report = temper_engine_io::block_on(report.recv()).expect("shutdown report");
    assert_eq!(
        report.pending_context_operations,
        BTreeSet::from([identity.clone()])
    );
    assert_eq!(report.blockers.len(), 1);
    let blocker = &report.blockers[0];
    assert_eq!(
        blocker.kind,
        temper_protocol_worker::ShutdownBlockerKind::ComponentTask
    );
    assert_eq!(blocker.worker_id.as_deref(), Some("worker-context"));
    assert_eq!(blocker.job_id.as_deref(), Some("job-context"));
    assert_eq!(blocker.attempt_id.as_deref(), Some("attempt-context"));
    assert_eq!(blocker.owner_name, "forge_context");

    machine.finish_context(admission);
    assert!(temper_engine_io::block_on(join_notification.recv()).is_some());
}

#[test]
fn shutdown_rejects_freshness_webhooks_and_armed_wake_work() {
    let mut machine = DaemonMachine::default_machine(Duration::ZERO);
    machine.webhook = Some(WebhookConfig {
        secret: "shutdown-secret".to_string(),
        targets: Vec::new(),
    });
    let repository = RepositoryPath::new("ai", "temper");
    let lane = WakeLane::Role(RoleId::new("engineer"));
    machine
        .wake_coordinator
        .configure_repository(repository.clone(), [lane.clone()]);
    let scheduled = machine.on_completion(
        EngineTime::ZERO,
        DaemonCompletion::ScheduleWake {
            request: WakeRequest::targeted_for_lanes(
                repository.clone(),
                [lane],
                HintArtifactKind::Issue,
                ItemNumber::new(660),
                ChangeKind::Label,
            ),
        },
    );
    let generation = scheduled
        .iter()
        .find_map(|request| match request {
            DaemonRequest::StartWakeTimer { generation, .. } => Some(*generation),
            _ => None,
        })
        .expect("wake timer is armed before shutdown");
    let (reply, _report) = oneshot();
    let (joined, _join_notification) = oneshot();
    machine.on_completion(
        EngineTime::ZERO,
        DaemonCompletion::BeginShutdown { reply, joined },
    );

    assert!(
        machine
            .on_completion(
                EngineTime::ZERO,
                DaemonCompletion::WakeTimerElapsed {
                    repo: repository,
                    generation,
                },
            )
            .is_empty(),
        "an armed timer cannot start wake work after fencing"
    );
    for uri in ["/v1/pr-freshness", "/forgejo/webhook"] {
        let (response, received) = oneshot();
        let requests = machine.on_completion(
            EngineTime::ZERO,
            DaemonCompletion::Http {
                request: HttpRequestData {
                    method: "POST".to_string(),
                    uri: uri.to_string(),
                    headers: Vec::new(),
                    body: Vec::new(),
                },
                responder: HttpResponder::from_oneshot(response),
                trusted_transport: false,
            },
        );
        assert!(matches!(
            requests.as_slice(),
            [DaemonRequest::Respond { response, .. }] if response.status == 503
        ));
        for request in requests {
            if let DaemonRequest::Respond {
                responder,
                response,
            } = request
            {
                responder.respond(response);
            }
        }
        assert_eq!(
            temper_engine_io::block_on(received.recv())
                .expect("fenced response")
                .status,
            503
        );
    }
}

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
        assert!(
            !requests.iter().any(|request| matches!(
                request,
                DaemonRequest::StartWakeTimer { .. } | DaemonRequest::RunWake { .. }
            )),
            "no timer or run starts during apply"
        );
        assert!(requests.iter().any(|request| matches!(
            request,
            DaemonRequest::WakeMeasurement(measurement)
                if measurement.outcome == "deferred"
        )));
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
fn wake_measurements_carry_stable_run_id_scope_counts_and_latencies() {
    let mut machine = DaemonMachine::default_machine(Duration::ZERO);
    let repository = RepositoryPath::new("ai", "temper");
    let lane = WakeLane::Role(RoleId::new("engineer"));
    machine
        .wake_coordinator
        .configure_repository(repository.clone(), [lane.clone()]);

    let scheduled = machine.on_completion(
        EngineTime::ZERO,
        DaemonCompletion::ScheduleWake {
            request: WakeRequest::targeted_for_lanes(
                repository.clone(),
                [lane],
                HintArtifactKind::Issue,
                ItemNumber::new(325),
                ChangeKind::Label,
            ),
        },
    );
    let accepted = scheduled
        .iter()
        .find_map(|request| match request {
            DaemonRequest::WakeMeasurement(measurement) => Some(measurement),
            _ => None,
        })
        .expect("accepted decision is measured");
    assert_eq!(accepted.repo, "ai/temper");
    assert_eq!(accepted.role.as_deref(), Some("engineer"));
    assert_eq!(accepted.reason, "label");
    assert_eq!(accepted.scope, "targeted");
    assert_eq!(accepted.outcome, "accepted");
    assert_eq!(accepted.pending_target_count, 1);
    let generation = scheduled
        .iter()
        .find_map(|request| match request {
            DaemonRequest::StartWakeTimer { generation, .. } => Some(*generation),
            _ => None,
        })
        .expect("timer is armed");

    let started = machine.on_completion(
        EngineTime::from_nanos(2_000_000),
        DaemonCompletion::WakeTimerElapsed {
            repo: repository,
            generation,
        },
    );
    let start_measurement = started
        .iter()
        .find_map(|request| match request {
            DaemonRequest::WakeMeasurement(measurement) => Some(measurement),
            _ => None,
        })
        .expect("start is measured");
    assert_eq!(start_measurement.run_id.as_deref(), Some("ai/temper:1"));
    assert_eq!(start_measurement.phase, "start");
    assert_eq!(start_measurement.queue_latency_ms, 2);
    assert_eq!(start_measurement.in_flight_repository_count, 1);
    let work = started
        .into_iter()
        .find_map(|request| match request {
            DaemonRequest::RunWake { work } => Some(work),
            _ => None,
        })
        .expect("wake work starts");

    let finished = machine.on_completion(
        EngineTime::from_nanos(7_000_000),
        DaemonCompletion::WakeFinished {
            work,
            outcome: WakeOutcome::Succeeded,
        },
    );
    let finish_measurement = finished
        .iter()
        .find_map(|request| match request {
            DaemonRequest::WakeMeasurement(measurement) if measurement.phase == "finish" => {
                Some(measurement)
            }
            _ => None,
        })
        .expect("completion is measured");
    assert_eq!(finish_measurement.run_id.as_deref(), Some("ai/temper:1"));
    assert_eq!(finish_measurement.outcome, "completed");
    assert_eq!(finish_measurement.execution_duration_ms, 5);
    assert_eq!(finish_measurement.in_flight_repository_count, 0);
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
