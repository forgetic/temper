//! The worker's pure sans-IO core.
//!
//! `WorkerMachine` is the sole authority for job liveness, terminal ordering,
//! heartbeat membership, and permit release. All time is supplied by the
//! runtime as `EngineTime`; timers are generation-tagged requests rather than a
//! watchdog thread.

use std::collections::BTreeMap;
use std::time::Duration;

use temper_protocol_worker::{Assign, ErrorCode, JobResult, WorkerProtocolMessage};
use temper_worker_io::{EngineTime, Machine};

use crate::agent_runner::JobProgress;
use crate::result_outbox::ResultOutboxEntry;

pub use crate::config::WorkerParams;

mod delivery;
mod watchdog;
pub use watchdog::{
    ActiveOperation, CancellationStatus, JobCleanup, JobPhase, JobWatchState, OperationId,
    OperationKind, ResultDeliveryStatus, ResultDurabilityStatus, TimeoutReason, TimeoutState,
    WatchdogTimerKind,
};

/// Read-only compatibility view over occupied job IDs.
pub struct InFlightJobs<'a>(&'a BTreeMap<String, JobWatchState>);

impl InFlightJobs<'_> {
    pub fn contains(&self, job_id: &str) -> bool {
        self.0.contains_key(job_id)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A finished I/O event delivered to the machine.
#[derive(Debug)]
pub enum WorkerCompletion {
    Registered(Result<(), String>),
    PollReply(Result<Option<WorkerProtocolMessage>, String>),
    JobProgress {
        job_id: String,
        attempt_id: String,
        generation: u64,
        progress: JobProgress,
    },
    WatchdogTimer {
        job_id: String,
        attempt_id: String,
        generation: u64,
        timer_generation: u64,
        kind: WatchdogTimerKind,
    },
    JobFinished {
        job_id: String,
        attempt_id: String,
        generation: u64,
        result: JobResult,
    },
    JobQuiesced {
        job_id: String,
        attempt_id: String,
        generation: u64,
        cleanup: JobCleanup,
    },
    ResultRecorded {
        job_id: String,
        attempt_id: String,
        generation: u64,
        outcome: Result<ResultOutboxEntry, String>,
    },
    ResultRecordTimer {
        job_id: String,
        attempt_id: String,
        generation: u64,
    },
    ResultDelivered {
        entry_id: String,
        outcome: Result<Option<WorkerProtocolMessage>, String>,
    },
    ResultFinalized {
        entry_id: String,
        outcome: Result<(), String>,
    },
    ResultReplayTimer {
        entry_id: String,
    },
    HeartbeatDelivered(Result<(), String>),
    PollTimer,
    HeartbeatTimer,
    Shutdown,
}

/// An I/O request the shell must perform.
#[derive(Debug)]
pub enum WorkerRequest {
    SendRegister(WorkerProtocolMessage),
    SendPoll(WorkerProtocolMessage),
    RunJob {
        assign: Assign,
        generation: u64,
    },
    CancelJob {
        job_id: String,
        attempt_id: String,
        generation: u64,
        reason: String,
    },
    EscalateJob {
        job_id: String,
        attempt_id: String,
        generation: u64,
        hard: bool,
    },
    ArmWatchdogTimer {
        job_id: String,
        attempt_id: String,
        generation: u64,
        timer_generation: u64,
        kind: WatchdogTimerKind,
        delay: Duration,
    },
    RecordResult {
        job_id: String,
        attempt_id: String,
        generation: u64,
        result: JobResult,
    },
    SendResult {
        entry_id: String,
        message: WorkerProtocolMessage,
    },
    AcknowledgeResult {
        entry: ResultOutboxEntry,
        release: temper_protocol_worker::Release,
    },
    RejectResult {
        entry: ResultOutboxEntry,
        reason: String,
    },
    SendHeartbeat(WorkerProtocolMessage),
    ArmResultRecordTimer {
        job_id: String,
        attempt_id: String,
        generation: u64,
        delay: Duration,
    },
    ArmResultReplayTimer {
        entry_id: String,
        delay: Duration,
    },
    ArmPollTimer(Duration),
    ArmHeartbeatTimer(Duration),
    Warn(String),
    Log(String),
}

pub struct WorkerMachine {
    params: WorkerParams,
    free_capacity: u32,
    jobs: BTreeMap<String, JobWatchState>,
    next_generation: u64,
    outbox: BTreeMap<String, ResultOutboxEntry>,
    replay_attempts: BTreeMap<String, u32>,
    registered: bool,
    stopped: bool,
}

impl WorkerMachine {
    pub fn new(params: WorkerParams) -> Self {
        Self::with_recovered_outbox(params, Vec::new())
    }

    pub fn with_recovered_outbox(params: WorkerParams, recovered: Vec<ResultOutboxEntry>) -> Self {
        let free_capacity = params.max_concurrent_jobs;
        let outbox = recovered
            .into_iter()
            .map(|entry| (entry.entry_id.clone(), entry))
            .collect();
        Self {
            params,
            free_capacity,
            jobs: BTreeMap::new(),
            next_generation: 1,
            outbox,
            replay_attempts: BTreeMap::new(),
            registered: false,
            stopped: false,
        }
    }

    pub fn free_capacity(&self) -> u32 {
        self.free_capacity
    }

    pub fn in_flight(&self) -> InFlightJobs<'_> {
        InFlightJobs(&self.jobs)
    }

    pub fn job_state(&self, job_id: &str) -> Option<&JobWatchState> {
        self.jobs.get(job_id)
    }

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

    fn on_poll_reply(
        &mut self,
        now: EngineTime,
        reply: Result<Option<WorkerProtocolMessage>, String>,
    ) -> Vec<WorkerRequest> {
        let mut requests = Vec::new();
        match reply {
            Ok(Some(WorkerProtocolMessage::Assign(assign))) => {
                let attempt_id = assign.attempt_id.clone().unwrap_or_default();
                if attempt_id.is_empty() {
                    requests.push(WorkerRequest::Log(format!(
                        "worker: refusing unfenced assignment job_id={}",
                        assign.job_id
                    )));
                    requests.push(WorkerRequest::ArmPollTimer(self.params.poll_backoff));
                } else if self.free_capacity == 0 || self.jobs.contains_key(&assign.job_id) {
                    requests.push(WorkerRequest::Log(format!(
                        "worker: refusing assignment job_id={} (free_capacity={}, already_in_flight={})",
                        assign.job_id,
                        self.free_capacity,
                        self.jobs.contains_key(&assign.job_id)
                    )));
                    requests.push(WorkerRequest::ArmPollTimer(self.params.poll_backoff));
                } else {
                    requests.push(WorkerRequest::Log(crate::observability::assigned_job_line(
                        &assign,
                    )));
                    self.free_capacity = self.free_capacity.saturating_sub(1);
                    let generation = self.next_generation;
                    self.next_generation = self.next_generation.saturating_add(1);
                    self.jobs.insert(
                        assign.job_id.clone(),
                        JobWatchState {
                            attempt_id: attempt_id.clone(),
                            generation,
                            phase: JobPhase::Running,
                            run_started_at: now,
                            last_agent_progress: now,
                            timer_generation: 1,
                            active_operations: BTreeMap::new(),
                            timeout: None,
                            cancellation: CancellationStatus::NotRequested,
                            escalation_requested: false,
                            result_durability: ResultDurabilityStatus::None,
                            result_delivery: ResultDeliveryStatus::NotReady,
                            last_progress_sequence: 0,
                            pending_result: None,
                        },
                    );
                    requests.push(WorkerRequest::RunJob {
                        assign: assign.clone(),
                        generation,
                    });
                    requests.push(self.no_progress_timer(&assign.job_id, now));
                    if let Some(delay) = self.params.liveness_limits.max_run {
                        requests.push(WorkerRequest::ArmWatchdogTimer {
                            job_id: assign.job_id.clone(),
                            attempt_id,
                            generation,
                            timer_generation: 0,
                            kind: WatchdogTimerKind::MaxRun,
                            delay,
                        });
                    }
                    requests.extend(self.poll_or_backoff());
                }
            }
            Ok(Some(WorkerProtocolMessage::Error(error)))
                if error.code == ErrorCode::PollTimeout =>
            {
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
        now: EngineTime,
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
            WorkerCompletion::Registered(Err(error)) => vec![
                WorkerRequest::Log(format!("worker: register failed: {error}")),
                WorkerRequest::ArmPollTimer(self.params.poll_backoff),
            ],
            WorkerCompletion::PollReply(reply) => self.on_poll_reply(now, reply),
            WorkerCompletion::PollTimer => {
                if self.registered {
                    self.poll_or_backoff()
                } else {
                    vec![WorkerRequest::SendRegister(
                        crate::client::register_message_params(&self.params),
                    )]
                }
            }
            WorkerCompletion::JobProgress {
                job_id,
                attempt_id,
                generation,
                progress,
            } => self.on_progress(now, job_id, attempt_id, generation, progress),
            WorkerCompletion::WatchdogTimer {
                job_id,
                attempt_id,
                generation,
                timer_generation,
                kind,
            } => {
                self.on_watchdog_timer(now, job_id, attempt_id, generation, timer_generation, kind)
            }
            WorkerCompletion::JobFinished {
                job_id,
                attempt_id,
                generation,
                result,
            } => {
                let Some(state) = self.jobs.get_mut(&job_id) else {
                    return Vec::new();
                };
                if state.phase != JobPhase::Running
                    || !state.accepts(&attempt_id, generation)
                    || result.attempt_id.as_deref() != Some(attempt_id.as_str())
                {
                    return Vec::new();
                }
                state.phase = JobPhase::Quiesced;
                state.cancellation = CancellationStatus::Quiesced;
                self.record_terminal(&job_id, &attempt_id, generation, result)
            }
            WorkerCompletion::JobQuiesced {
                job_id,
                attempt_id,
                generation,
                cleanup,
            } => {
                let Some(state) = self.jobs.get_mut(&job_id) else {
                    return Vec::new();
                };
                if state.phase != JobPhase::CancelRequested
                    || !state.accepts(&attempt_id, generation)
                {
                    return Vec::new();
                }
                state.phase = JobPhase::Quiesced;
                state.cancellation = CancellationStatus::Quiesced;
                let state = state.clone();
                let result = self.timeout_result(&job_id, &state, now, &cleanup);
                self.record_terminal(&job_id, &attempt_id, generation, result)
            }
            WorkerCompletion::ResultRecorded {
                job_id,
                attempt_id,
                generation,
                outcome,
            } => {
                let Some(state) = self.jobs.get(&job_id) else {
                    return Vec::new();
                };
                if !state.accepts(&attempt_id, generation)
                    || state.phase != JobPhase::Quiesced
                    || state.result_durability != ResultDurabilityStatus::Pending
                {
                    return Vec::new();
                }
                match outcome {
                    Ok(entry) => {
                        if let Some(state) = self.jobs.get_mut(&job_id) {
                            state.phase = JobPhase::ResultRecorded;
                            state.result_durability = ResultDurabilityStatus::Durable;
                            state.result_delivery = ResultDeliveryStatus::Pending;
                        }
                        self.jobs.remove(&job_id);
                        self.free_capacity = self
                            .free_capacity
                            .saturating_add(1)
                            .min(self.params.max_concurrent_jobs);
                        self.outbox.insert(entry.entry_id.clone(), entry.clone());
                        let mut requests = vec![
                            WorkerRequest::Log(crate::observability::result_sent_line(
                                &entry.result,
                            )),
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
                            attempt_id,
                            generation,
                            delay: self.params.poll_backoff,
                        },
                    ],
                }
            }
            WorkerCompletion::ResultRecordTimer {
                job_id,
                attempt_id,
                generation,
            } => self
                .jobs
                .get(&job_id)
                .filter(|state| {
                    state.accepts(&attempt_id, generation)
                        && state.phase == JobPhase::Quiesced
                        && state.result_durability == ResultDurabilityStatus::Pending
                })
                .and_then(|state| state.pending_result.clone())
                .map(|result| {
                    vec![WorkerRequest::RecordResult {
                        job_id,
                        attempt_id,
                        generation,
                        result,
                    }]
                })
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
                if !self.jobs.is_empty() {
                    requests.push(self.heartbeat_request());
                }
                requests.push(WorkerRequest::ArmHeartbeatTimer(
                    self.params.heartbeat_interval,
                ));
                requests
            }
            WorkerCompletion::HeartbeatDelivered(Err(error)) => {
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

#[cfg(test)]
#[path = "worker_machine_watchdog_tests.rs"]
mod watchdog_tests;
