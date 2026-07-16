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
//! [`temper_worker_io::drive_sync`]: feed a completion sequence and assert on the
//! emitted requests, with no runtime and no races. The production shell is also
//! driven under `temper-sim`'s skein-lab runtime for high-fidelity worker/daemon
//! scenarios over in-process transport.

use std::collections::{BTreeMap, BTreeSet};

use temper_protocol_worker::{
    Assign, ErrorCode, JobResult, ReleaseDisposition, WorkerProtocolMessage,
};
use temper_worker_io::{EngineTime, Machine};

use crate::result_outbox::ResultOutboxEntry;

pub use crate::config::WorkerParams;

/// A finished I/O event delivered to the machine.
#[derive(Debug)]
pub enum WorkerCompletion {
    /// The register POST completed (`Ok` = accepted, `Err` = transport error).
    Registered(Result<(), String>),
    /// A poll POST completed, yielding the daemon's reply (or a transport
    /// error). `Ok(None)` is an empty/204 reply.
    PollReply(Result<Option<WorkerProtocolMessage>, String>),
    /// A dispatched job finished; its exact result must be durably recorded
    /// before local capacity is released or delivery begins.
    JobFinished { job_id: String, result: JobResult },
    /// One outbox record operation completed.
    ResultRecorded {
        job_id: String,
        outcome: Result<ResultOutboxEntry, String>,
    },
    /// Retry an outbox record that previously failed.
    ResultRecordTimer { job_id: String },
    /// A result POST completed with the daemon's exact protocol reply.
    ResultDelivered {
        entry_id: String,
        outcome: Result<Option<WorkerProtocolMessage>, String>,
    },
    /// Durable acknowledgement compaction or permanent rejection completed.
    ResultFinalized {
        entry_id: String,
        outcome: Result<(), String>,
    },
    /// A per-entry replay timer fired.
    ResultReplayTimer { entry_id: String },
    /// A heartbeat POST completed.
    HeartbeatDelivered(Result<(), String>),
    /// The poll-backoff timer fired: time to poll again.
    PollTimer,
    /// The heartbeat cadence timer fired: time to heartbeat (if work in flight).
    HeartbeatTimer,
    /// Stop the component loop without reporting or releasing in-flight work.
    /// Test restart harnesses use this to model a process crash deterministically.
    Shutdown,
}

/// An I/O request the shell must perform.
#[derive(Debug)]
pub enum WorkerRequest {
    /// POST a register message; completes as [`WorkerCompletion::Registered`].
    SendRegister(WorkerProtocolMessage),
    /// POST a poll message; completes as [`WorkerCompletion::PollReply`].
    SendPoll(WorkerProtocolMessage),
    /// Persist an exact result in the crash-safe outbox.
    RecordResult { job_id: String, result: JobResult },
    /// POST a recorded result; completes as [`WorkerCompletion::ResultDelivered`].
    SendResult {
        entry_id: String,
        message: WorkerProtocolMessage,
    },
    /// Delete an entry after a matching release acknowledgement.
    AcknowledgeResult {
        entry: ResultOutboxEntry,
        release: temper_protocol_worker::Release,
    },
    /// Move a permanently rejected entry to operator-visible rejected storage.
    RejectResult {
        entry: ResultOutboxEntry,
        reason: String,
    },
    /// POST a heartbeat; completes as [`WorkerCompletion::HeartbeatDelivered`].
    SendHeartbeat(WorkerProtocolMessage),
    /// Run an assigned job; completes as [`WorkerCompletion::JobFinished`].
    RunJob(Assign),
    /// Retry an outbox record without coupling it to polling or permits.
    ArmResultRecordTimer {
        job_id: String,
        delay: std::time::Duration,
    },
    /// Retry one durable result independently of job permit availability.
    ArmResultReplayTimer {
        entry_id: String,
        delay: std::time::Duration,
    },
    /// Arm the poll-backoff timer; completes as [`WorkerCompletion::PollTimer`].
    ArmPollTimer(std::time::Duration),
    /// Arm the heartbeat cadence timer; completes as
    /// [`WorkerCompletion::HeartbeatTimer`].
    ArmHeartbeatTimer(std::time::Duration),
    /// Operator-visible warning for stale/rejected durable result delivery.
    Warn(String),
    /// A human-facing debug log line (observability; the shell prints it).
    Log(String),
}

/// The pure worker core.
pub struct WorkerMachine {
    params: WorkerParams,
    free_capacity: u32,
    in_flight: BTreeSet<String>,
    in_flight_attempts: BTreeMap<String, String>,
    pending_records: BTreeMap<String, JobResult>,
    outbox: BTreeMap<String, ResultOutboxEntry>,
    replay_attempts: BTreeMap<String, u32>,
    registered: bool,
    stopped: bool,
}

impl WorkerMachine {
    pub fn new(params: WorkerParams) -> Self {
        Self::with_recovered_outbox(params, Vec::new())
    }

    /// Constructs a worker with entries recovered by the startup outbox scan.
    pub fn with_recovered_outbox(params: WorkerParams, recovered: Vec<ResultOutboxEntry>) -> Self {
        let free_capacity = params.max_concurrent_jobs;
        let outbox = recovered
            .into_iter()
            .map(|entry| (entry.entry_id.clone(), entry))
            .collect();
        Self {
            params,
            free_capacity,
            in_flight: BTreeSet::new(),
            in_flight_attempts: BTreeMap::new(),
            pending_records: BTreeMap::new(),
            outbox,
            replay_attempts: BTreeMap::new(),
            registered: false,
            stopped: false,
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
                if assign.attempt_id.as_deref().is_none_or(str::is_empty) {
                    requests.push(WorkerRequest::Log(format!(
                        "worker: refusing unfenced assignment job_id={}",
                        assign.job_id
                    )));
                    requests.push(WorkerRequest::ArmPollTimer(self.params.poll_backoff));
                } else if self.free_capacity == 0 || self.in_flight.contains(&assign.job_id) {
                    requests.push(WorkerRequest::Log(format!(
                        "worker: refusing assignment job_id={} (free_capacity={}, already_in_flight={})",
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
                    self.in_flight_attempts.insert(
                        assign.job_id.clone(),
                        assign.attempt_id.clone().expect("attempt checked above"),
                    );
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
                    "worker: unexpected poll reply from daemon: {other:?}"
                )));
                requests.push(WorkerRequest::ArmPollTimer(self.params.poll_backoff));
            }
            Ok(None) => {
                requests.push(WorkerRequest::Log(
                    "worker: empty poll reply from daemon".to_string(),
                ));
                requests.push(WorkerRequest::ArmPollTimer(self.params.poll_backoff));
            }
            Err(error) => {
                requests.push(WorkerRequest::Log(format!("worker: poll failed: {error}")));
                requests.push(WorkerRequest::ArmPollTimer(self.params.poll_backoff));
            }
        }
        requests
    }

    fn send_entry(entry: &ResultOutboxEntry) -> WorkerRequest {
        WorkerRequest::SendResult {
            entry_id: entry.entry_id.clone(),
            message: WorkerProtocolMessage::Result(entry.result.clone()),
        }
    }

    fn retry_entry(&mut self, entry_id: &str, reason: String) -> Vec<WorkerRequest> {
        if !self.outbox.contains_key(entry_id) {
            return Vec::new();
        }
        let attempt = self
            .replay_attempts
            .entry(entry_id.to_string())
            .or_insert(0);
        *attempt = attempt.saturating_add(1);
        let exponent = attempt.saturating_sub(1).min(8);
        let delay = std::time::Duration::from_secs(2_u64.saturating_pow(exponent).min(300));
        vec![
            WorkerRequest::Log(format!(
                "worker: retaining durable result entry_id={entry_id} retry={} backoff_ms={} reason={reason}",
                *attempt,
                delay.as_millis()
            )),
            WorkerRequest::ArmResultReplayTimer {
                entry_id: entry_id.to_string(),
                delay,
            },
        ]
    }

    fn result_delivery(
        &mut self,
        entry_id: String,
        outcome: Result<Option<WorkerProtocolMessage>, String>,
    ) -> Vec<WorkerRequest> {
        let Some(entry) = self.outbox.get(&entry_id).cloned() else {
            return Vec::new();
        };
        match outcome {
            Ok(Some(WorkerProtocolMessage::Release(release)))
                if entry.matches_release(&release) =>
            {
                if matches!(
                    release.disposition,
                    ReleaseDisposition::Superseded | ReleaseDisposition::Reclaimed
                ) {
                    return vec![
                        WorkerRequest::Warn(format!(
                            "worker: durable result became stale entry_id={} job_id={} attempt_id={} disposition={:?}",
                            entry.entry_id,
                            entry.assignment.job_id,
                            entry.assignment.attempt_id,
                            release.disposition
                        )),
                        WorkerRequest::AcknowledgeResult { entry, release },
                    ];
                }
                vec![WorkerRequest::AcknowledgeResult { entry, release }]
            }
            Ok(Some(WorkerProtocolMessage::Error(error)))
                if matches!(
                    error.code,
                    ErrorCode::Unauthorized
                        | ErrorCode::MalformedMessage
                        | ErrorCode::ProtocolVersionMismatch
                        | ErrorCode::RegistrationRejected
                ) =>
            {
                vec![
                    WorkerRequest::Warn(format!(
                        "worker: permanently rejecting durable result entry_id={} job_id={} attempt_id={} code={:?}",
                        entry.entry_id,
                        entry.assignment.job_id,
                        entry.assignment.attempt_id,
                        error.code
                    )),
                    WorkerRequest::RejectResult {
                        entry,
                        reason: format!("daemon permanently rejected result: {:?}", error.code),
                    },
                ]
            }
            Ok(Some(other)) => self.retry_entry(
                &entry_id,
                format!("unexpected daemon acknowledgement: {other:?}"),
            ),
            Ok(None) => self.retry_entry(
                &entry_id,
                "daemon returned no release acknowledgement".to_string(),
            ),
            Err(error) if permanent_transport_rejection(&error) => {
                vec![
                    WorkerRequest::Warn(format!(
                        "worker: permanently rejecting durable result entry_id={} job_id={} attempt_id={} reason={error}",
                        entry.entry_id, entry.assignment.job_id, entry.assignment.attempt_id,
                    )),
                    WorkerRequest::RejectResult {
                        entry,
                        reason: error,
                    },
                ]
            }
            Err(error) => self.retry_entry(&entry_id, error),
        }
    }
}

fn permanent_transport_rejection(error: &str) -> bool {
    [
        "HTTP 400", "HTTP 401", "HTTP 403", "HTTP 404", "HTTP 409", "HTTP 422",
    ]
    .iter()
    .any(|status| error.contains(status))
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
                        self.params.worker_pool.as_deref(),
                        self.params.max_concurrent_jobs,
                        &self.params.capabilities,
                    ),
                )];
                requests.extend(self.outbox.values().map(Self::send_entry));
                requests.extend(self.poll_or_backoff());
                requests
            }
            WorkerCompletion::Registered(Err(error)) => {
                // Registration is required before work; back off and retry.
                vec![
                    WorkerRequest::Log(format!("worker: register failed: {error}")),
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
                if !self.in_flight.contains(&job_id) || self.pending_records.contains_key(&job_id) {
                    return Vec::new();
                }
                self.pending_records.insert(job_id.clone(), result.clone());
                vec![WorkerRequest::RecordResult { job_id, result }]
            }
            WorkerCompletion::ResultRecorded { job_id, outcome } => match outcome {
                Ok(entry) => {
                    self.pending_records.remove(&job_id);
                    self.in_flight.remove(&job_id);
                    self.in_flight_attempts.remove(&job_id);
                    self.free_capacity = self
                        .free_capacity
                        .saturating_add(1)
                        .min(self.params.max_concurrent_jobs);
                    self.outbox.insert(entry.entry_id.clone(), entry.clone());
                    let mut requests = vec![
                        WorkerRequest::Log(crate::observability::result_sent_line(&entry.result)),
                        Self::send_entry(&entry),
                    ];
                    requests.extend(self.poll_or_backoff());
                    requests
                }
                Err(error) => vec![
                    WorkerRequest::Log(format!(
                        "worker: durable result recording failed for job {job_id}: {error}"
                    )),
                    WorkerRequest::ArmResultRecordTimer {
                        job_id,
                        delay: self.params.poll_backoff,
                    },
                ],
            },
            WorkerCompletion::ResultRecordTimer { job_id } => self
                .pending_records
                .get(&job_id)
                .cloned()
                .map(|result| vec![WorkerRequest::RecordResult { job_id, result }])
                .unwrap_or_default(),
            WorkerCompletion::ResultDelivered { entry_id, outcome } => {
                self.result_delivery(entry_id, outcome)
            }
            WorkerCompletion::ResultReplayTimer { entry_id } => self
                .outbox
                .get(&entry_id)
                .map(Self::send_entry)
                .into_iter()
                .collect(),
            WorkerCompletion::ResultFinalized { entry_id, outcome } => match outcome {
                Ok(()) => {
                    self.outbox.remove(&entry_id);
                    self.replay_attempts.remove(&entry_id);
                    Vec::new()
                }
                Err(error) => self.retry_entry(&entry_id, error),
            },
            WorkerCompletion::HeartbeatTimer => {
                let mut requests = Vec::new();
                if !self.in_flight.is_empty() {
                    requests.push(WorkerRequest::SendHeartbeat(
                        crate::client::heartbeat_message_params_attempts(
                            &self.params,
                            &self.in_flight_attempts,
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
                // A daemon replacement forgets process-local registrations but
                // the worker still owns its in-flight job set. Re-register and
                // keep that set intact so the next exact heartbeat can reattach
                // any durable assignment staged by startup recovery.
                self.registered = false;
                vec![
                    WorkerRequest::Log(format!("worker: heartbeat failed: {error}")),
                    WorkerRequest::SendRegister(crate::client::register_message_params(
                        &self.params,
                    )),
                ]
            }
            WorkerCompletion::HeartbeatDelivered(Ok(())) => Vec::new(),
            WorkerCompletion::Shutdown => {
                self.stopped = true;
                Vec::new()
            }
        }
    }

    fn is_stopped(&self) -> bool {
        self.stopped
    }
}

#[cfg(test)]
#[path = "worker_machine_tests.rs"]
mod tests;
