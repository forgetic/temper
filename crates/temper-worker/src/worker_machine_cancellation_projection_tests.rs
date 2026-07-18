//! Projection tests for joined cancellation reports.

use std::time::Duration;

use temper_process_containment::{CleanupPhase, CleanupSnapshot, CleanupTrigger};
use temper_protocol_worker::{FailureClass, WorkerProtocolMessage};
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
fn cleanup_blocked_retains_fence_permit_and_rejects_unproven_completion() {
    let job_id = "job-cleanup-blocked";
    let attempt_id = format!("attempt-{job_id}");
    let mut machine = WorkerMachine::new(params());
    dispatch_at(&mut machine, job_id, EngineTime::ZERO);
    let generation = machine.job_state(job_id).unwrap().generation;

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

    let mut cleanup = JobCleanup::no_process(None);
    cleanup.resources.stderr_reader =
        ResourceJoinStatus::Failed("injected reader join failure".to_string());
    let result = job_result_for_attempt(
        "worker-1",
        job_id,
        Some(format!("attempt-{job_id}")),
        JobOutcome::Failure {
            class: FailureClass::Transient,
            message: "fixture".to_string(),
        },
    );
    let rejected = machine.on_completion(
        EngineTime::from_nanos(2),
        WorkerCompletion::AttemptQuiesced {
            job_id: job_id.to_string(),
            attempt_id,
            generation,
            completion: AttemptCompletion {
                result: Some(result),
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
