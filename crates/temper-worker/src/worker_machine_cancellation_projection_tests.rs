//! Projection tests for joined cancellation reports.

use std::time::Duration;

use temper_process_containment::{CleanupPhase, CleanupSnapshot, CleanupTrigger};
use temper_protocol_worker::{
    AttemptCancellation, CancelAttempts, FailureClass, WorkerProtocolMessage,
};
use temper_worker_io::{EngineTime, Machine};

use super::tests::{assign, params};
use super::{
    AttemptCompletion, JobCleanup, WatchdogTimerKind, WorkerCompletion, WorkerMachine,
    WorkerRequest,
};
use crate::executor::{
    CancellationOutcome, JobOutcome, ResourceJoinStatus, job_result_for_attempt,
};

fn dispatch_at(machine: &mut WorkerMachine, job_id: &str, now: EngineTime) -> Vec<WorkerRequest> {
    machine.on_completion(
        now,
        WorkerCompletion::PollReply(Ok(Some(WorkerProtocolMessage::Assign(assign(job_id))))),
    )
}

fn request_ownership_loss(
    machine: &mut WorkerMachine,
    job_id: &str,
    reason: &str,
) -> Vec<WorkerRequest> {
    let directive = CancelAttempts::new(
        "worker-1",
        vec![
            AttemptCancellation::ownership_lost(
                "worker-1",
                job_id,
                format!("attempt-{job_id}"),
                reason,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    machine.on_completion(
        EngineTime::from_nanos(1),
        WorkerCompletion::HeartbeatDelivered(Ok(Some(directive))),
    )
}

#[test]
fn real_cancellation_and_cleanup_outcomes_project_without_synthesis() {
    let cases = [
        (CancellationOutcome::Graceful, "graceful", false),
        (
            CancellationOutcome::ForcedTermination,
            "forced_termination",
            true,
        ),
        (CancellationOutcome::HardKill, "hard_kill", true),
    ];

    for (index, (cancellation, expected_outcome, forced)) in cases.into_iter().enumerate() {
        let job_id = format!("job-projection-{index}");
        let attempt_id = format!("attempt-{job_id}");
        let mut config = params();
        config.liveness_limits.max_no_progress = Duration::from_nanos(10);
        let mut machine = WorkerMachine::new(config);
        dispatch_at(&mut machine, &job_id, EngineTime::ZERO);
        let state = machine.job_state(&job_id).unwrap();
        let generation = state.generation;
        let timer_generation = state.timer_generation;
        machine.on_completion(
            EngineTime::from_nanos(11),
            WorkerCompletion::WatchdogTimer {
                job_id: job_id.clone(),
                attempt_id: attempt_id.clone(),
                generation,
                timer_generation,
                kind: WatchdogTimerKind::NoProgress,
            },
        );

        let requests = machine.on_completion(
            EngineTime::from_nanos(12),
            WorkerCompletion::AttemptQuiesced {
                job_id: job_id.clone(),
                attempt_id,
                generation,
                completion: AttemptCompletion {
                    result: None,
                    cleanup: JobCleanup::no_process(Some(cancellation)),
                },
            },
        );
        assert!(requests.iter().any(|request| matches!(
            request,
            WorkerRequest::Observe(crate::observability::WorkerEvent::CancellationCompleted {
                outcome,
                descendant_cleanup,
                forced: observed_forced,
                ..
            }) if outcome == expected_outcome
                && descendant_cleanup == "AlreadyEmpty"
                && *observed_forced == forced
        )));
        let result = requests
            .iter()
            .find_map(|request| match request {
                WorkerRequest::RecordResult { result, .. } => Some(result),
                _ => None,
            })
            .expect("real cancellation outcome creates timeout result");
        let cleanup = &result.details.as_ref().unwrap()["timeout"]["cleanup"];
        assert_eq!(cleanup["cancellation"], expected_outcome);
        assert_eq!(cleanup["backend"], "no_process");
        assert_eq!(cleanup["disposition"], "already_empty");
        assert_eq!(cleanup["recursive_empty"], true);
    }
}

#[test]
fn heartbeat_rejects_a_cancellation_envelope_for_another_worker() {
    let job_id = "job-wrong-worker";
    let mut machine = WorkerMachine::new(params());
    dispatch_at(&mut machine, job_id, EngineTime::ZERO);
    let directive = CancelAttempts::new(
        "worker-2",
        vec![
            AttemptCancellation::ownership_lost(
                "worker-2",
                job_id,
                format!("attempt-{job_id}"),
                "not this worker",
            )
            .unwrap(),
        ],
    )
    .unwrap();

    let requests = machine.on_completion(
        EngineTime::ZERO,
        WorkerCompletion::HeartbeatDelivered(Ok(Some(directive))),
    );

    assert_eq!(
        machine.job_state(job_id).unwrap().phase,
        super::JobPhase::Running
    );
    assert!(
        requests
            .iter()
            .all(|request| !matches!(request, WorkerRequest::CancelJob { .. }))
    );
    assert!(
        requests
            .iter()
            .any(|request| matches!(request, WorkerRequest::SendRegister(_)))
    );
}

#[test]
fn ownership_loss_records_canceled_cleanup_evidence_before_releasing_capacity() {
    let job_id = "job-ownership-result";
    let attempt_id = format!("attempt-{job_id}");
    let mut machine = WorkerMachine::new(params());
    dispatch_at(&mut machine, job_id, EngineTime::ZERO);
    let generation = machine.job_state(job_id).unwrap().generation;
    request_ownership_loss(
        &mut machine,
        job_id,
        "assignment belongs to a newer attempt",
    );

    let late_result = job_result_for_attempt(
        "worker-1",
        job_id,
        Some(attempt_id.clone()),
        JobOutcome::Failure {
            class: FailureClass::Permanent,
            message: "late executor completion".to_string(),
        },
    );
    let record = machine.on_completion(
        EngineTime::from_nanos(2),
        WorkerCompletion::AttemptQuiesced {
            job_id: job_id.to_string(),
            attempt_id: attempt_id.clone(),
            generation,
            completion: AttemptCompletion {
                result: Some(late_result),
                cleanup: JobCleanup::no_process(Some(CancellationOutcome::HardKill)),
            },
        },
    );
    assert_eq!(machine.free_capacity(), 0);
    assert!(machine.in_flight().contains(job_id));
    let result = record
        .iter()
        .find_map(|request| match request {
            WorkerRequest::RecordResult { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("ownership-loss result must be durably recorded");
    let failure = result.failure.as_ref().expect("canceled failure");
    assert_eq!(failure.class, FailureClass::Canceled);
    assert!(
        failure
            .message
            .contains("assignment belongs to a newer attempt")
    );
    assert_eq!(
        result.details.as_ref().unwrap()["cancellation"]["cause"],
        "ownership_lost"
    );
    let cleanup = &result.details.as_ref().unwrap()["cancellation"]["cleanup"];
    assert_eq!(cleanup["cancellation"], "hard_kill");
    assert_eq!(cleanup["recursive_empty"], true);
    assert_eq!(
        cleanup["resources"]["forge_endpoint"]["status"],
        "not_applicable"
    );

    let failed_record = machine.on_completion(
        EngineTime::from_nanos(3),
        WorkerCompletion::ResultRecorded {
            job_id: job_id.to_string(),
            attempt_id: attempt_id.clone(),
            generation,
            outcome: Err("outbox unavailable".to_string()),
        },
    );
    assert_eq!(machine.free_capacity(), 0);
    assert!(machine.in_flight().contains(job_id));
    assert_no_terminal_or_release(&failed_record);

    let entry = crate::result_outbox::ResultOutboxEntry::from_result(result).unwrap();
    let released = machine.on_completion(
        EngineTime::from_nanos(4),
        WorkerCompletion::ResultRecorded {
            job_id: job_id.to_string(),
            attempt_id,
            generation,
            outcome: Ok(entry),
        },
    );
    assert_eq!(machine.free_capacity(), 1);
    assert!(!machine.in_flight().contains(job_id));
    assert!(released.iter().any(|request| matches!(
        request,
        WorkerRequest::Observe(crate::observability::WorkerEvent::CapacityReleased { .. })
    )));
}

#[test]
fn cleanup_blocked_retains_fence_permit_and_rejects_unproven_completion() {
    let job_id = "job-cleanup-blocked";
    let attempt_id = format!("attempt-{job_id}");
    let mut machine = WorkerMachine::new(params());
    dispatch_at(&mut machine, job_id, EngineTime::ZERO);
    let generation = machine.job_state(job_id).unwrap().generation;
    request_ownership_loss(
        &mut machine,
        job_id,
        "durable assignment was removed while running",
    );

    let blocked = machine.on_completion(
        EngineTime::from_nanos(1),
        WorkerCompletion::AttemptCleanupBlocked {
            job_id: job_id.to_string(),
            attempt_id: attempt_id.clone(),
            generation,
            snapshot: CleanupSnapshot::Blocked {
                trigger: CleanupTrigger::NormalRootExit,
                phase: CleanupPhase::VerifyEmpty,
                message: "injected membership inspection failure".to_string(),
                survivors: Vec::new(),
                omitted_survivors: 0,
            },
        },
    );
    let state = machine
        .job_state(job_id)
        .expect("attempt remains installed");
    assert_eq!(state.phase, super::JobPhase::CleanupPending);
    assert_eq!(
        state.cancellation,
        super::CancellationStatus::CleanupBlocked
    );
    assert_eq!(machine.free_capacity(), 0);
    assert_no_terminal_or_release(&blocked);

    let mut cleanup = JobCleanup::no_process(Some(CancellationOutcome::Graceful));
    cleanup.resources.stderr_reader =
        ResourceJoinStatus::Failed("injected reader join failure".to_string());
    let rejected = machine.on_completion(
        EngineTime::from_nanos(2),
        WorkerCompletion::AttemptQuiesced {
            job_id: job_id.to_string(),
            attempt_id,
            generation,
            completion: AttemptCompletion {
                result: None,
                cleanup,
            },
        },
    );
    assert_eq!(machine.free_capacity(), 0);
    assert_eq!(
        machine.job_state(job_id).unwrap().phase,
        super::JobPhase::CleanupPending
    );
    assert_no_terminal_or_release(&rejected);
}

fn assert_no_terminal_or_release(requests: &[WorkerRequest]) {
    assert!(!requests.iter().any(|request| matches!(
        request,
        WorkerRequest::RecordResult { .. }
            | WorkerRequest::SendPoll(_)
            | WorkerRequest::Observe(crate::observability::WorkerEvent::CapacityReleased { .. })
    )));
}
