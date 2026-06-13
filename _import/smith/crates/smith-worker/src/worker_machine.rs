//! The worker's pure sans-IO core.
//!
//! [`WorkerMachine`] is the deterministic logic of the long-poll worker:
//! register, poll the daemon for work while capacity is free, dispatch assigned
//! jobs, report results, and heartbeat in-flight jobs. It performs no I/O — it
//! consumes [`WorkerCompletion`]s (a poll reply arrived, a job finished, a timer
//! fired) and emits [`WorkerRequest`]s (send this message, run this job, arm
//! this timer). The imperative shell ([`crate::worker_shell`]) performs the
//! actual HTTP/agent/timer work and feeds results back.
//!
//! Because it is pure, the whole worker control flow — the poll/dispatch/result
//! interleavings the tokio `select!` loop used to hide — is unit-testable with
//! [`smith_io_engine::drive_sync`]: feed a completion sequence and assert on the
//! emitted requests, with no runtime and no races. That is also what makes it a
//! target for skein-lab simulation later.

use std::collections::BTreeSet;

use smith_io_engine::{EngineTime, Machine};
use temper_worker_protocol::{Assign, ErrorCode, JobResult, WorkerProtocolMessage};

pub use crate::config::WorkerParams;

/// A finished I/O event delivered to the machine.
#[derive(Debug)]
pub enum WorkerCompletion {
    /// The register POST completed (`Ok` = accepted, `Err` = transport error).
    Registered(Result<(), String>),
    /// A poll POST completed, yielding the daemon's reply (or a transport
    /// error). `Ok(None)` is an empty/204 reply.
    PollReply(Result<Option<WorkerProtocolMessage>, String>),
    /// A dispatched job finished; its result is ready to report.
    JobFinished { job_id: String, result: JobResult },
    /// A result POST completed (`Err` = transport error; the machine logs and
    /// moves on — the daemon re-leases on its own timeout).
    ResultDelivered {
        job_id: String,
        outcome: Result<(), String>,
    },
    /// A heartbeat POST completed.
    HeartbeatDelivered(Result<(), String>),
    /// The poll-backoff timer fired: time to poll again.
    PollTimer,
    /// The heartbeat cadence timer fired: time to heartbeat (if work in flight).
    HeartbeatTimer,
}

/// An I/O request the shell must perform.
#[derive(Debug)]
pub enum WorkerRequest {
    /// POST a register message; completes as [`WorkerCompletion::Registered`].
    SendRegister(WorkerProtocolMessage),
    /// POST a poll message; completes as [`WorkerCompletion::PollReply`].
    SendPoll(WorkerProtocolMessage),
    /// POST a result; completes as [`WorkerCompletion::ResultDelivered`].
    SendResult {
        job_id: String,
        message: WorkerProtocolMessage,
    },
    /// POST a heartbeat; completes as [`WorkerCompletion::HeartbeatDelivered`].
    SendHeartbeat(WorkerProtocolMessage),
    /// Run an assigned job; completes as [`WorkerCompletion::JobFinished`].
    RunJob(Assign),
    /// Arm the poll-backoff timer; completes as [`WorkerCompletion::PollTimer`].
    ArmPollTimer(std::time::Duration),
    /// Arm the heartbeat cadence timer; completes as
    /// [`WorkerCompletion::HeartbeatTimer`].
    ArmHeartbeatTimer(std::time::Duration),
    /// A human-facing log line (observability; the shell prints it). Keeps the
    /// machine pure while still emitting the same operational log contract the
    /// old loop did.
    Log(String),
}

/// The pure worker core.
pub struct WorkerMachine {
    params: WorkerParams,
    free_capacity: u32,
    in_flight: BTreeSet<String>,
    /// Set once the worker has registered; gates the first poll.
    registered: bool,
}

impl WorkerMachine {
    pub fn new(params: WorkerParams) -> Self {
        let free_capacity = params.max_concurrent_jobs;
        Self {
            params,
            free_capacity,
            in_flight: BTreeSet::new(),
            registered: false,
        }
    }

    /// Free capacity right now (test/observability accessor).
    pub fn free_capacity(&self) -> u32 {
        self.free_capacity
    }

    /// In-flight job ids right now (test/observability accessor).
    pub fn in_flight(&self) -> &BTreeSet<String> {
        &self.in_flight
    }

    /// Poll the daemon if there is free capacity, else arm the backoff timer so
    /// we re-poll once a job frees a slot or the timer elapses. Centralizes the
    /// "should I poll now?" decision the old `select!` guard encoded.
    fn poll_or_backoff(&self) -> Vec<WorkerRequest> {
        if self.free_capacity > 0 {
            vec![WorkerRequest::SendPoll(crate::client::poll_message_params(
                &self.params,
                self.free_capacity,
            ))]
        } else {
            vec![WorkerRequest::ArmPollTimer(self.params.poll_backoff)]
        }
    }

    /// Handle one poll reply: dispatch an assignment, ignore a poll-timeout, or
    /// surface an unexpected message — then decide the next poll.
    fn on_poll_reply(
        &mut self,
        reply: Result<Option<WorkerProtocolMessage>, String>,
    ) -> Vec<WorkerRequest> {
        let mut requests = Vec::new();
        match reply {
            Ok(Some(WorkerProtocolMessage::Assign(assign))) => {
                // Defensive: the machine only polls with free capacity, so the
                // daemon should never assign when we are full or re-assign a job
                // already in flight. If it does (a buggy/racing daemon), refuse
                // rather than over-subscribe or double-run — capacity
                // conservation is an invariant, not a hope. Back off and re-sync
                // on the next poll.
                if self.free_capacity == 0 || self.in_flight.contains(&assign.job_id) {
                    requests.push(WorkerRequest::Log(format!(
                        "smith-worker: refusing assignment job_id={} (free_capacity={}, already_in_flight={})",
                        assign.job_id,
                        self.free_capacity,
                        self.in_flight.contains(&assign.job_id)
                    )));
                    requests.push(WorkerRequest::ArmPollTimer(self.params.poll_backoff));
                } else {
                    requests.push(WorkerRequest::Log(crate::observability::assigned_job_line(
                        &assign,
                    )));
                    self.free_capacity = self.free_capacity.saturating_sub(1);
                    self.in_flight.insert(assign.job_id.clone());
                    requests.push(WorkerRequest::RunJob(assign));
                    // Immediately try to poll again — more work may be waiting
                    // and we may still have capacity.
                    requests.extend(self.poll_or_backoff());
                }
            }
            Ok(Some(WorkerProtocolMessage::Error(error)))
                if error.code == ErrorCode::PollTimeout =>
            {
                // Long-poll elapsed with no work; back off before re-polling.
                requests.push(WorkerRequest::ArmPollTimer(self.params.poll_backoff));
            }
            Ok(Some(other)) => {
                requests.push(WorkerRequest::Log(format!(
                    "smith-worker: unexpected poll reply from daemon: {other:?}"
                )));
                requests.push(WorkerRequest::ArmPollTimer(self.params.poll_backoff));
            }
            Ok(None) => {
                requests.push(WorkerRequest::Log(
                    "smith-worker: empty poll reply from daemon".to_string(),
                ));
                requests.push(WorkerRequest::ArmPollTimer(self.params.poll_backoff));
            }
            Err(error) => {
                requests.push(WorkerRequest::Log(format!(
                    "smith-worker: poll failed: {error}"
                )));
                requests.push(WorkerRequest::ArmPollTimer(self.params.poll_backoff));
            }
        }
        requests
    }
}

impl Machine for WorkerMachine {
    type Completion = WorkerCompletion;
    type Request = WorkerRequest;

    fn on_start(&mut self, _now: EngineTime) -> Vec<WorkerRequest> {
        vec![
            WorkerRequest::SendRegister(crate::client::register_message_params(&self.params)),
            WorkerRequest::ArmHeartbeatTimer(self.params.heartbeat_interval),
        ]
    }

    fn on_completion(
        &mut self,
        _now: EngineTime,
        completion: WorkerCompletion,
    ) -> Vec<WorkerRequest> {
        match completion {
            WorkerCompletion::Registered(Ok(())) => {
                self.registered = true;
                let mut requests = vec![WorkerRequest::Log(
                    crate::observability::registered_worker_line(
                        &self.params.worker_id,
                        self.params.capabilities.len(),
                    ),
                )];
                requests.extend(self.poll_or_backoff());
                requests
            }
            WorkerCompletion::Registered(Err(error)) => {
                // Registration is required before work; back off and retry.
                vec![
                    WorkerRequest::Log(format!("smith-worker: register failed: {error}")),
                    WorkerRequest::ArmPollTimer(self.params.poll_backoff),
                ]
            }
            WorkerCompletion::PollReply(reply) => self.on_poll_reply(reply),
            WorkerCompletion::PollTimer => {
                // Re-poll if registered; otherwise retry registration.
                if self.registered {
                    self.poll_or_backoff()
                } else {
                    vec![WorkerRequest::SendRegister(
                        crate::client::register_message_params(&self.params),
                    )]
                }
            }
            WorkerCompletion::JobFinished { job_id, result } => {
                self.in_flight.remove(&job_id);
                self.free_capacity = self
                    .free_capacity
                    .saturating_add(1)
                    .min(self.params.max_concurrent_jobs);
                let mut requests = vec![
                    WorkerRequest::Log(crate::observability::result_sent_line(&result)),
                    WorkerRequest::SendResult {
                        job_id,
                        message: WorkerProtocolMessage::Result(result),
                    },
                ];
                // A slot just freed; poll again right away.
                requests.extend(self.poll_or_backoff());
                requests
            }
            WorkerCompletion::ResultDelivered { job_id, outcome } => match outcome {
                Ok(()) => Vec::new(),
                Err(error) => vec![WorkerRequest::Log(format!(
                    "smith-worker: result delivery failed for job {job_id}: {error}"
                ))],
            },
            WorkerCompletion::HeartbeatTimer => {
                let mut requests = Vec::new();
                if !self.in_flight.is_empty() {
                    requests.push(WorkerRequest::SendHeartbeat(
                        crate::client::heartbeat_message_params(
                            &self.params,
                            &self.in_flight,
                            self.free_capacity,
                        ),
                    ));
                }
                // Re-arm the cadence regardless, so heartbeats resume when work
                // arrives.
                requests.push(WorkerRequest::ArmHeartbeatTimer(
                    self.params.heartbeat_interval,
                ));
                requests
            }
            WorkerCompletion::HeartbeatDelivered(Err(error)) => {
                vec![WorkerRequest::Log(format!(
                    "smith-worker: heartbeat failed: {error}"
                ))]
            }
            WorkerCompletion::HeartbeatDelivered(Ok(())) => Vec::new(),
        }
    }
}

#[cfg(test)]
#[path = "worker_machine_tests.rs"]
mod tests;
