//! Deterministic, runtime-free unit tests for [`WorkerMachine`].
//!
//! Each test feeds a synthetic completion sequence and asserts on the emitted
//! requests — the poll/dispatch/result/heartbeat interleavings the old
//! `tokio::select!` loop hid are now plain, replayable function calls.

use std::cell::RefCell;
use std::time::Duration;

use serde_json::json;
use smith_io_engine::{EngineTime, Machine, drive_sync};
use temper_worker_protocol::{
    Artifact, Assign, ErrorCode, ProtocolError, ResultStatus, WORKER_PROTOCOL_VERSION,
    WorkerProtocolMessage,
};

use super::{WorkerCompletion, WorkerMachine, WorkerRequest};
use crate::config::{CapabilitySpec, WorkerParams};
use crate::executor::{JobOutcome, job_result};

fn params() -> WorkerParams {
    WorkerParams {
        worker_id: "worker-1".to_string(),
        capabilities: vec![CapabilitySpec {
            repo: "ai/smith".to_string(),
            role: "engineer".to_string(),
        }],
        max_concurrent_jobs: 1,
        poll_wait: Duration::from_millis(100),
        heartbeat_interval: Duration::from_millis(50),
        poll_backoff: Duration::from_millis(500),
    }
}

fn assign(job_id: &str) -> Assign {
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: job_id.to_string(),
        role: "engineer".to_string(),
        repo: "ai/smith".to_string(),
        artifact: Artifact {
            item: json!(1),
            kind: "issue".to_string(),
        },
        job_payload: json!({}),
    }
}

fn poll_timeout() -> WorkerProtocolMessage {
    WorkerProtocolMessage::Error(ProtocolError {
        protocol_version: WORKER_PROTOCOL_VERSION,
        code: ErrorCode::PollTimeout,
        message: "no work".to_string(),
        retry_after_ms: None,
        job_id: None,
    })
}

/// A recording executor for `drive_sync`: captures every request and lets a test
/// assert on the sequence.
#[derive(Default)]
struct Recorder {
    requests: RefCell<Vec<WorkerRequest>>,
}

impl smith_io_engine::Executor<WorkerMachine> for Recorder {
    fn execute(&self, request: WorkerRequest) {
        self.requests.borrow_mut().push(request);
    }
}

/// Drive the machine over a completion sequence, returning the captured requests.
fn run(machine: &mut WorkerMachine, completions: Vec<WorkerCompletion>) -> Vec<WorkerRequest> {
    let recorder = Recorder::default();
    drive_sync(machine, &recorder, completions, || EngineTime::ZERO);
    recorder.requests.into_inner()
}

#[test]
fn on_start_registers_and_arms_heartbeat() {
    let mut machine = WorkerMachine::new(params());
    let requests = machine.on_start(EngineTime::ZERO);
    assert!(matches!(requests[0], WorkerRequest::SendRegister(_)));
    assert!(matches!(
        requests[1],
        WorkerRequest::ArmHeartbeatTimer(d) if d == Duration::from_millis(50)
    ));
}

#[test]
fn register_ack_logs_and_polls() {
    let mut machine = WorkerMachine::new(params());
    machine.on_start(EngineTime::ZERO);
    let requests = run(&mut machine, vec![WorkerCompletion::Registered(Ok(()))]);
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, WorkerRequest::Log(line) if line.contains("registered"))),
        "expected a registered log line, got {requests:?}"
    );
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, WorkerRequest::SendPoll(_))),
        "expected a poll after registering, got {requests:?}"
    );
}

#[test]
fn assign_reply_dispatches_job_and_decrements_capacity() {
    let mut machine = WorkerMachine::new(params());
    machine.on_start(EngineTime::ZERO);
    // Register, then a poll reply carrying an assignment.
    let requests = run(
        &mut machine,
        vec![
            WorkerCompletion::Registered(Ok(())),
            WorkerCompletion::PollReply(Ok(Some(WorkerProtocolMessage::Assign(assign("job-1"))))),
        ],
    );
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, WorkerRequest::RunJob(a) if a.job_id == "job-1")),
        "expected the job to be dispatched, got {requests:?}"
    );
    assert_eq!(machine.free_capacity(), 0);
    assert!(machine.in_flight().contains("job-1"));
    // At capacity now, so the follow-up poll must be a backoff timer, not a poll.
    let tail = &requests[requests.len() - 1];
    assert!(
        matches!(tail, WorkerRequest::ArmPollTimer(_)),
        "expected a backoff timer once at capacity, got {tail:?}"
    );
}

#[test]
fn assignment_is_refused_when_already_at_capacity() {
    // max_concurrent = 1; take a job, then a (misbehaving) daemon assigns
    // another while we are full. The machine must refuse rather than
    // over-subscribe.
    let mut machine = WorkerMachine::new(params());
    machine.on_start(EngineTime::ZERO);
    run(
        &mut machine,
        vec![
            WorkerCompletion::Registered(Ok(())),
            WorkerCompletion::PollReply(Ok(Some(WorkerProtocolMessage::Assign(assign("job-1"))))),
        ],
    );
    assert_eq!(machine.free_capacity(), 0);

    let requests = run(
        &mut machine,
        vec![WorkerCompletion::PollReply(Ok(Some(
            WorkerProtocolMessage::Assign(assign("job-2")),
        )))],
    );
    // No second dispatch; capacity conserved; logged the refusal + backed off.
    assert!(
        !requests
            .iter()
            .any(|r| matches!(r, WorkerRequest::RunJob(a) if a.job_id == "job-2")),
        "must not dispatch a second job at capacity: {requests:?}"
    );
    assert_eq!(machine.free_capacity(), 0);
    assert_eq!(machine.in_flight().len(), 1);
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, WorkerRequest::Log(line) if line.contains("refusing assignment"))),
        "expected a refusal log: {requests:?}"
    );
}

#[test]
fn duplicate_assignment_of_in_flight_job_is_refused() {
    let mut machine = WorkerMachine::new(WorkerParams {
        max_concurrent_jobs: 4,
        ..params()
    });
    machine.on_start(EngineTime::ZERO);
    run(
        &mut machine,
        vec![
            WorkerCompletion::Registered(Ok(())),
            WorkerCompletion::PollReply(Ok(Some(WorkerProtocolMessage::Assign(assign("job-1"))))),
        ],
    );
    assert_eq!(machine.in_flight().len(), 1);

    // The same job id is assigned again (daemon race). Refuse — don't double-run.
    let requests = run(
        &mut machine,
        vec![WorkerCompletion::PollReply(Ok(Some(
            WorkerProtocolMessage::Assign(assign("job-1")),
        )))],
    );
    assert!(
        !requests
            .iter()
            .any(|r| matches!(r, WorkerRequest::RunJob(_))),
        "must not re-dispatch an in-flight job: {requests:?}"
    );
    assert_eq!(machine.in_flight().len(), 1);
    // free_capacity unchanged (still 3 of 4).
    assert_eq!(machine.free_capacity(), 3);
}

#[test]
fn poll_timeout_arms_backoff_without_logging_error() {
    let mut machine = WorkerMachine::new(params());
    machine.on_start(EngineTime::ZERO);
    let requests = run(
        &mut machine,
        vec![
            WorkerCompletion::Registered(Ok(())),
            WorkerCompletion::PollReply(Ok(Some(poll_timeout()))),
        ],
    );
    // The timeout is normal: no error log, just a backoff timer.
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, WorkerRequest::ArmPollTimer(_))),
        "expected a backoff timer after poll timeout, got {requests:?}"
    );
    assert!(
        !requests
            .iter()
            .any(|r| matches!(r, WorkerRequest::Log(line) if line.contains("unexpected"))),
        "poll timeout must not log an unexpected-reply error: {requests:?}"
    );
}

#[test]
fn job_finished_reports_result_frees_capacity_and_repolls() {
    let mut machine = WorkerMachine::new(params());
    machine.on_start(EngineTime::ZERO);
    // Register -> assign -> job runs and finishes.
    run(
        &mut machine,
        vec![
            WorkerCompletion::Registered(Ok(())),
            WorkerCompletion::PollReply(Ok(Some(WorkerProtocolMessage::Assign(assign("job-1"))))),
        ],
    );
    let result = job_result(
        "worker-1",
        "job-1",
        JobOutcome::Failure {
            class: temper_worker_protocol::FailureClass::Permanent,
            message: "nope".to_string(),
        },
    );
    let requests = run(
        &mut machine,
        vec![WorkerCompletion::JobFinished {
            job_id: "job-1".to_string(),
            result,
        }],
    );
    assert_eq!(machine.free_capacity(), 1);
    assert!(machine.in_flight().is_empty());
    assert!(
        requests.iter().any(|r| matches!(
            r,
            WorkerRequest::SendResult { message: WorkerProtocolMessage::Result(result), .. }
                if result.status == ResultStatus::Failure
        )),
        "expected a result POST, got {requests:?}"
    );
    // Capacity freed, so we poll again (not a backoff timer).
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, WorkerRequest::SendPoll(_))),
        "expected a re-poll after a slot freed, got {requests:?}"
    );
}

#[test]
fn heartbeat_timer_sends_only_when_work_in_flight_and_rearms() {
    let mut machine = WorkerMachine::new(params());
    machine.on_start(EngineTime::ZERO);
    run(&mut machine, vec![WorkerCompletion::Registered(Ok(()))]);

    // Idle: heartbeat timer re-arms but sends nothing.
    let idle = run(&mut machine, vec![WorkerCompletion::HeartbeatTimer]);
    assert!(
        !idle
            .iter()
            .any(|r| matches!(r, WorkerRequest::SendHeartbeat(_))),
        "idle worker must not heartbeat: {idle:?}"
    );
    assert!(
        idle.iter()
            .any(|r| matches!(r, WorkerRequest::ArmHeartbeatTimer(_))),
        "heartbeat cadence must re-arm even when idle: {idle:?}"
    );

    // With a job in flight, the heartbeat fires.
    run(
        &mut machine,
        vec![WorkerCompletion::PollReply(Ok(Some(
            WorkerProtocolMessage::Assign(assign("job-1")),
        )))],
    );
    let busy = run(&mut machine, vec![WorkerCompletion::HeartbeatTimer]);
    assert!(
        busy.iter()
            .any(|r| matches!(r, WorkerRequest::SendHeartbeat(_))),
        "busy worker must heartbeat: {busy:?}"
    );
}

#[test]
fn poll_transport_error_backs_off_and_logs() {
    let mut machine = WorkerMachine::new(params());
    machine.on_start(EngineTime::ZERO);
    run(&mut machine, vec![WorkerCompletion::Registered(Ok(()))]);
    let requests = run(
        &mut machine,
        vec![WorkerCompletion::PollReply(Err(
            "connection refused".to_string()
        ))],
    );
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, WorkerRequest::Log(line) if line.contains("poll failed"))),
        "expected a poll-failure log, got {requests:?}"
    );
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, WorkerRequest::ArmPollTimer(_))),
        "expected a backoff timer after a transport error, got {requests:?}"
    );
}
