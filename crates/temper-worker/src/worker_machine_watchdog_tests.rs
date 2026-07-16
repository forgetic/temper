//! Deadline, race, and capacity tests for the per-job watchdog.

use std::time::Duration;

use temper_protocol_agent::{
    AGENT_LIFECYCLE_PROTOCOL_VERSION, AgentLifecycleEventV1, AgentLifecycleFrameV1,
    AgentLifecycleScopeV1,
};
use temper_protocol_worker::WorkerProtocolMessage;
use temper_worker_io::{EngineTime, Machine};

use super::tests::{assign, params};
use super::{
    JobCleanup, JobPhase, TimeoutReason, WatchdogTimerKind, WorkerCompletion, WorkerMachine,
    WorkerRequest,
};
use crate::agent_runner::JobProgress;
use crate::executor::{JobOutcome, job_result_for_attempt};
use crate::result_outbox::ResultOutboxEntry;

fn dispatch_at(machine: &mut WorkerMachine, job_id: &str, now: EngineTime) -> Vec<WorkerRequest> {
    machine.on_completion(
        now,
        WorkerCompletion::PollReply(Ok(Some(WorkerProtocolMessage::Assign(assign(job_id))))),
    )
}

fn lifecycle_progress(
    attempt_id: &str,
    received_at: EngineTime,
    seq: u64,
    event: AgentLifecycleEventV1,
) -> JobProgress {
    JobProgress {
        attempt_id: attempt_id.to_string(),
        received_at,
        frame: AgentLifecycleFrameV1 {
            version: AGENT_LIFECYCLE_PROTOCOL_VERSION,
            seq,
            scope: AgentLifecycleScopeV1 {
                id: "main".to_string(),
                parent_id: None,
            },
            event,
        },
    }
}

fn terminal_result(job_id: &str) -> temper_protocol_worker::JobResult {
    job_result_for_attempt(
        "worker-1",
        job_id,
        Some(format!("attempt-{job_id}")),
        JobOutcome::Failure {
            class: temper_protocol_worker::FailureClass::Permanent,
            message: "finished".to_string(),
        },
    )
}

#[test]
fn exact_boundary_progress_wins_and_stale_no_progress_timers_are_ignored() {
    let mut config = params();
    config.liveness_limits.max_no_progress = Duration::from_nanos(10);
    let mut machine = WorkerMachine::new(config);
    dispatch_at(&mut machine, "job-boundary", EngineTime::ZERO);
    let state = machine.job_state("job-boundary").unwrap();
    let generation = state.generation;
    let initial_timer_generation = state.timer_generation;

    let equality = machine.on_completion(
        EngineTime::from_nanos(10),
        WorkerCompletion::WatchdogTimer {
            job_id: "job-boundary".to_string(),
            attempt_id: "attempt-job-boundary".to_string(),
            generation,
            timer_generation: initial_timer_generation,
            kind: WatchdogTimerKind::NoProgress,
        },
    );
    assert!(equality.iter().any(|request| matches!(
        request,
        WorkerRequest::ArmWatchdogTimer { delay, .. } if *delay == Duration::from_nanos(1)
    )));
    assert_eq!(
        machine.job_state("job-boundary").unwrap().phase,
        JobPhase::Running
    );

    let progress = lifecycle_progress(
        "attempt-job-boundary",
        EngineTime::from_nanos(10),
        1,
        AgentLifecycleEventV1::ToolStarted {
            call_id: "tool-1".to_string(),
            name: "forge_list_related".to_string(),
        },
    );
    machine.on_completion(
        EngineTime::from_nanos(10),
        WorkerCompletion::JobProgress {
            job_id: "job-boundary".to_string(),
            attempt_id: "attempt-job-boundary".to_string(),
            generation,
            progress,
        },
    );
    let state = machine.job_state("job-boundary").unwrap();
    assert_eq!(state.last_agent_progress, EngineTime::from_nanos(10));
    assert_eq!(state.timer_generation, initial_timer_generation + 1);
    assert_eq!(state.active_operations.len(), 1);

    let stale = machine.on_completion(
        EngineTime::from_nanos(21),
        WorkerCompletion::WatchdogTimer {
            job_id: "job-boundary".to_string(),
            attempt_id: "attempt-job-boundary".to_string(),
            generation,
            timer_generation: initial_timer_generation,
            kind: WatchdogTimerKind::NoProgress,
        },
    );
    assert!(stale.is_empty());
    assert_eq!(
        machine.job_state("job-boundary").unwrap().phase,
        JobPhase::Running
    );
}

#[test]
fn no_progress_timeout_quiesces_records_once_then_releases_capacity() {
    let mut config = params();
    config.liveness_limits.max_no_progress = Duration::from_nanos(10);
    config.liveness_limits.graceful_cancellation_grace = Duration::from_nanos(2);
    config.liveness_limits.forced_termination_grace = Duration::from_nanos(2);
    let mut machine = WorkerMachine::new(config);
    dispatch_at(&mut machine, "job-timeout", EngineTime::ZERO);
    let state = machine.job_state("job-timeout").unwrap();
    let generation = state.generation;
    let timer_generation = state.timer_generation;

    let timeout = machine.on_completion(
        EngineTime::from_nanos(11),
        WorkerCompletion::WatchdogTimer {
            job_id: "job-timeout".to_string(),
            attempt_id: "attempt-job-timeout".to_string(),
            generation,
            timer_generation,
            kind: WatchdogTimerKind::NoProgress,
        },
    );
    assert_eq!(
        machine.job_state("job-timeout").unwrap().phase,
        JobPhase::CancelRequested
    );
    assert_eq!(
        machine
            .job_state("job-timeout")
            .unwrap()
            .timeout
            .as_ref()
            .unwrap()
            .reason,
        TimeoutReason::NoProgress
    );
    assert_eq!(
        timeout
            .iter()
            .filter(|request| matches!(request, WorkerRequest::CancelJob { .. }))
            .count(),
        1
    );
    let finishing = timeout.iter().find_map(|request| match request {
        WorkerRequest::SendHeartbeat(WorkerProtocolMessage::Heartbeat(heartbeat)) => {
            Some(heartbeat)
        }
        _ => None,
    });
    assert!(finishing.unwrap().jobs.iter().any(|job| {
        job.job_id == "job-timeout"
            && job.state == temper_protocol_worker::HeartbeatState::Finishing
    }));

    // Re-delivery of the winning timer cannot request cancellation twice.
    let duplicate = machine.on_completion(
        EngineTime::from_nanos(12),
        WorkerCompletion::WatchdogTimer {
            job_id: "job-timeout".to_string(),
            attempt_id: "attempt-job-timeout".to_string(),
            generation,
            timer_generation,
            kind: WatchdogTimerKind::NoProgress,
        },
    );
    assert!(duplicate.is_empty());

    let escalation = machine.on_completion(
        EngineTime::from_nanos(13),
        WorkerCompletion::WatchdogTimer {
            job_id: "job-timeout".to_string(),
            attempt_id: "attempt-job-timeout".to_string(),
            generation,
            timer_generation: 0,
            kind: WatchdogTimerKind::CancellationGrace,
        },
    );
    assert_eq!(
        escalation
            .iter()
            .filter(|request| matches!(request, WorkerRequest::EscalateJob { hard: false, .. }))
            .count(),
        1
    );

    let record = machine.on_completion(
        EngineTime::from_nanos(14),
        WorkerCompletion::JobQuiesced {
            job_id: "job-timeout".to_string(),
            attempt_id: "attempt-job-timeout".to_string(),
            generation,
            cleanup: JobCleanup {
                cancellation: "graceful".to_string(),
                descendants: "joined".to_string(),
            },
        },
    );
    assert_eq!(machine.free_capacity(), 0, "recording precedes release");
    let result = record
        .iter()
        .find_map(|request| match request {
            WorkerRequest::RecordResult { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("timeout result is recorded");
    assert_eq!(
        result.failure.as_ref().unwrap().class,
        temper_protocol_worker::FailureClass::Transient
    );
    assert_eq!(
        result.details.as_ref().unwrap()["timeout"]["reason"],
        "no_progress"
    );
    assert_eq!(
        result.details.as_ref().unwrap()["timeout"]["cleanup"]["escalated"],
        true
    );

    // Late normal completion is fenced by phase and cannot replace the timeout.
    assert!(
        machine
            .on_completion(
                EngineTime::from_nanos(14),
                WorkerCompletion::JobFinished {
                    job_id: "job-timeout".to_string(),
                    attempt_id: "attempt-job-timeout".to_string(),
                    generation,
                    result: terminal_result("job-timeout"),
                },
            )
            .is_empty()
    );

    let entry = ResultOutboxEntry::from_result(result).unwrap();
    let released = machine.on_completion(
        EngineTime::from_nanos(15),
        WorkerCompletion::ResultRecorded {
            job_id: "job-timeout".to_string(),
            attempt_id: "attempt-job-timeout".to_string(),
            generation,
            outcome: Ok(entry),
        },
    );
    assert_eq!(machine.free_capacity(), 1);
    assert!(machine.in_flight().is_empty());
    assert_eq!(
        released
            .iter()
            .filter(|request| matches!(request, WorkerRequest::SendResult { .. }))
            .count(),
        1
    );
    assert!(
        released
            .iter()
            .any(|request| matches!(request, WorkerRequest::SendPoll(_)))
    );
}

#[test]
fn normal_completion_beats_timeout_and_duplicate_completion_releases_once() {
    let mut config = params();
    config.liveness_limits.max_no_progress = Duration::from_nanos(10);
    let mut machine = WorkerMachine::new(config);
    dispatch_at(&mut machine, "job-race", EngineTime::ZERO);
    let state = machine.job_state("job-race").unwrap();
    let generation = state.generation;
    let timer_generation = state.timer_generation;
    let result = terminal_result("job-race");

    let first = machine.on_completion(
        EngineTime::from_nanos(11),
        WorkerCompletion::JobFinished {
            job_id: "job-race".to_string(),
            attempt_id: "attempt-job-race".to_string(),
            generation,
            result: result.clone(),
        },
    );
    assert!(
        first
            .iter()
            .any(|request| matches!(request, WorkerRequest::RecordResult { .. }))
    );
    assert!(
        machine
            .on_completion(
                EngineTime::from_nanos(11),
                WorkerCompletion::WatchdogTimer {
                    job_id: "job-race".to_string(),
                    attempt_id: "attempt-job-race".to_string(),
                    generation,
                    timer_generation,
                    kind: WatchdogTimerKind::NoProgress,
                },
            )
            .is_empty()
    );
    assert!(
        machine
            .on_completion(
                EngineTime::from_nanos(12),
                WorkerCompletion::JobFinished {
                    job_id: "job-race".to_string(),
                    attempt_id: "attempt-job-race".to_string(),
                    generation,
                    result: result.clone(),
                },
            )
            .is_empty()
    );

    let entry = ResultOutboxEntry::from_result(result).unwrap();
    machine.on_completion(
        EngineTime::from_nanos(13),
        WorkerCompletion::ResultRecorded {
            job_id: "job-race".to_string(),
            attempt_id: "attempt-job-race".to_string(),
            generation,
            outcome: Ok(entry.clone()),
        },
    );
    assert_eq!(machine.free_capacity(), 1);
    assert!(
        machine
            .on_completion(
                EngineTime::from_nanos(14),
                WorkerCompletion::ResultRecorded {
                    job_id: "job-race".to_string(),
                    attempt_id: "attempt-job-race".to_string(),
                    generation,
                    outcome: Ok(entry),
                },
            )
            .is_empty()
    );
    assert_eq!(machine.free_capacity(), 1);
}

#[test]
fn max_run_is_independent_of_progress_and_releasing_one_of_many_preserves_membership() {
    let mut config = params();
    config.max_concurrent_jobs = 2;
    config.liveness_limits.max_no_progress = Duration::from_secs(60);
    config.liveness_limits.max_run = Some(Duration::from_nanos(20));
    let mut machine = WorkerMachine::new(config);
    dispatch_at(&mut machine, "job-a", EngineTime::ZERO);
    dispatch_at(&mut machine, "job-b", EngineTime::from_nanos(1));
    assert_eq!(machine.free_capacity(), 0);
    let state_a = machine.job_state("job-a").unwrap();
    let generation_a = state_a.generation;

    machine.on_completion(
        EngineTime::from_nanos(19),
        WorkerCompletion::JobProgress {
            job_id: "job-a".to_string(),
            attempt_id: "attempt-job-a".to_string(),
            generation: generation_a,
            progress: lifecycle_progress(
                "attempt-job-a",
                EngineTime::from_nanos(19),
                1,
                AgentLifecycleEventV1::SteeringApplied,
            ),
        },
    );
    let at_boundary = machine.on_completion(
        EngineTime::from_nanos(20),
        WorkerCompletion::WatchdogTimer {
            job_id: "job-a".to_string(),
            attempt_id: "attempt-job-a".to_string(),
            generation: generation_a,
            timer_generation: 0,
            kind: WatchdogTimerKind::MaxRun,
        },
    );
    assert!(at_boundary.iter().any(|request| matches!(
        request,
        WorkerRequest::ArmWatchdogTimer { delay, .. } if *delay == Duration::from_nanos(1)
    )));
    machine.on_completion(
        EngineTime::from_nanos(21),
        WorkerCompletion::WatchdogTimer {
            job_id: "job-a".to_string(),
            attempt_id: "attempt-job-a".to_string(),
            generation: generation_a,
            timer_generation: 0,
            kind: WatchdogTimerKind::MaxRun,
        },
    );
    let record = machine.on_completion(
        EngineTime::from_nanos(22),
        WorkerCompletion::JobQuiesced {
            job_id: "job-a".to_string(),
            attempt_id: "attempt-job-a".to_string(),
            generation: generation_a,
            cleanup: JobCleanup {
                cancellation: "forced".to_string(),
                descendants: "joined".to_string(),
            },
        },
    );
    let result = record
        .iter()
        .find_map(|request| match request {
            WorkerRequest::RecordResult { result, .. } => Some(result.clone()),
            _ => None,
        })
        .unwrap();
    let entry = ResultOutboxEntry::from_result(result).unwrap();
    machine.on_completion(
        EngineTime::from_nanos(23),
        WorkerCompletion::ResultRecorded {
            job_id: "job-a".to_string(),
            attempt_id: "attempt-job-a".to_string(),
            generation: generation_a,
            outcome: Ok(entry),
        },
    );
    assert_eq!(machine.free_capacity(), 1);
    assert!(!machine.in_flight().contains("job-a"));
    assert!(machine.in_flight().contains("job-b"));

    let heartbeat =
        machine.on_completion(EngineTime::from_nanos(24), WorkerCompletion::HeartbeatTimer);
    let jobs = heartbeat
        .iter()
        .find_map(|request| match request {
            WorkerRequest::SendHeartbeat(WorkerProtocolMessage::Heartbeat(heartbeat)) => {
                Some(&heartbeat.jobs)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].job_id, "job-b");
}
