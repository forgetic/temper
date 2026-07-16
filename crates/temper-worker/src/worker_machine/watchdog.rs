//! Per-job watchdog state and deadline/cancellation transitions.

use std::collections::BTreeMap;
use std::time::Duration;

use temper_protocol_agent::AgentLifecycleEventV1;
use temper_protocol_worker::{
    Failure, FailureClass, HeartbeatState, JobCancellationState, JobHeartbeat, JobHeartbeatPhase,
    JobLiveness, JobOperationKind, JobOperationSummary, JobResult, JobResultDeliveryState,
    JobResultDurabilityState, JobTimeoutReason, JobTimeoutSummary, MAX_ACTIVE_OPERATION_SUMMARIES,
    ResultStatus, WORKER_PROTOCOL_VERSION,
};
use temper_worker_io::EngineTime;

use crate::agent_runner::JobProgress;

use super::{JobCleanup, WorkerMachine, WorkerRequest};

/// Smallest re-arm used at an exact deadline. Timeout is strictly `now >
/// deadline`, so a completion stamped at the deadline always has a chance to
/// win.
const MIN_TIMER_TICK: Duration = Duration::from_nanos(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobPhase {
    Running,
    CancelRequested,
    Quiesced,
    ResultRecorded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    Model,
    Tool,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct OperationId(pub String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveOperation {
    pub scope: String,
    pub kind: OperationKind,
    pub name: String,
    pub id: String,
    pub started_at: EngineTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeoutReason {
    NoProgress,
    MaxRun,
}

impl TimeoutReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoProgress => "no_progress",
            Self::MaxRun => "max_run",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeoutState {
    pub reason: TimeoutReason,
    pub limit: Duration,
    pub operation: Option<ActiveOperation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellationStatus {
    NotRequested,
    Requested,
    Escalated,
    Quiesced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultDurabilityStatus {
    None,
    Pending,
    Durable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultDeliveryStatus {
    NotReady,
    Pending,
}

/// All worker-owned state for one occupied permit.
#[derive(Clone, Debug)]
pub struct JobWatchState {
    pub attempt_id: String,
    pub generation: u64,
    pub phase: JobPhase,
    pub run_started_at: EngineTime,
    pub last_agent_progress: EngineTime,
    pub timer_generation: u64,
    pub active_operations: BTreeMap<OperationId, ActiveOperation>,
    pub timeout: Option<TimeoutState>,
    pub cancellation: CancellationStatus,
    pub escalation_requested: bool,
    pub result_durability: ResultDurabilityStatus,
    pub result_delivery: ResultDeliveryStatus,
    pub(super) last_progress_sequence: u64,
    pub(super) pending_result: Option<JobResult>,
}

impl JobWatchState {
    fn current_operation(&self) -> Option<ActiveOperation> {
        self.active_operations
            .values()
            .min_by(|left, right| {
                left.started_at
                    .cmp(&right.started_at)
                    .then_with(|| left.id.cmp(&right.id))
            })
            .cloned()
    }

    pub(super) fn accepts(&self, attempt_id: &str, generation: u64) -> bool {
        self.attempt_id == attempt_id && self.generation == generation
    }

    fn heartbeat(&self, job_id: &str, now: EngineTime) -> JobHeartbeat {
        let state = if self.phase == JobPhase::Running {
            HeartbeatState::Running
        } else {
            HeartbeatState::Finishing
        };
        let active_operation_count =
            u32::try_from(self.active_operations.len()).unwrap_or(u32::MAX);
        let active_operations = self
            .active_operations
            .values()
            .take(MAX_ACTIVE_OPERATION_SUMMARIES)
            .map(|operation| JobOperationSummary {
                scope: operation.scope.clone(),
                kind: match operation.kind {
                    OperationKind::Model => JobOperationKind::Model,
                    OperationKind::Tool => JobOperationKind::Tool,
                },
                name: operation.name.clone(),
                operation_id: operation.id.clone(),
                elapsed_ms: elapsed_ms(now, operation.started_at),
            })
            .collect();
        JobHeartbeat {
            job_id: job_id.to_string(),
            attempt_id: Some(self.attempt_id.clone()),
            state,
            message: match self.phase {
                JobPhase::Running => "running",
                JobPhase::CancelRequested => "cancelling",
                JobPhase::Quiesced => "recording_result",
                JobPhase::ResultRecorded => "result_recorded",
            }
            .to_string(),
            liveness: Some(JobLiveness {
                phase: match self.phase {
                    JobPhase::Running => JobHeartbeatPhase::Running,
                    JobPhase::CancelRequested => JobHeartbeatPhase::CancelRequested,
                    JobPhase::Quiesced => JobHeartbeatPhase::Quiesced,
                    JobPhase::ResultRecorded => JobHeartbeatPhase::ResultRecorded,
                },
                run_elapsed_ms: elapsed_ms(now, self.run_started_at),
                no_progress_elapsed_ms: elapsed_ms(now, self.last_agent_progress),
                active_operation_count,
                active_operations,
                timeout: self.timeout.as_ref().map(|timeout| JobTimeoutSummary {
                    reason: match timeout.reason {
                        TimeoutReason::NoProgress => JobTimeoutReason::NoProgress,
                        TimeoutReason::MaxRun => JobTimeoutReason::MaxRun,
                    },
                    limit_ms: duration_ms(timeout.limit),
                }),
                cancellation: match self.cancellation {
                    CancellationStatus::NotRequested => JobCancellationState::NotRequested,
                    CancellationStatus::Requested => JobCancellationState::Requested,
                    CancellationStatus::Escalated => JobCancellationState::Escalated,
                    CancellationStatus::Quiesced => JobCancellationState::Quiesced,
                },
                result_durability: match self.result_durability {
                    ResultDurabilityStatus::None => JobResultDurabilityState::None,
                    ResultDurabilityStatus::Pending => JobResultDurabilityState::Pending,
                    ResultDurabilityStatus::Durable => JobResultDurabilityState::Durable,
                },
                result_delivery: match self.result_delivery {
                    ResultDeliveryStatus::NotReady => JobResultDeliveryState::NotReady,
                    ResultDeliveryStatus::Pending => JobResultDeliveryState::Pending,
                },
                pending_result: self.pending_result.is_some(),
            }),
        }
    }

    fn update_operations(&mut self, progress: &JobProgress) {
        let scope = &progress.frame.scope.id;
        match &progress.frame.event {
            AgentLifecycleEventV1::ModelStarted { call_id, .. } => {
                let id = OperationId(format!("{scope}:model:{call_id}"));
                self.active_operations.insert(
                    id,
                    ActiveOperation {
                        scope: scope.clone(),
                        kind: OperationKind::Model,
                        name: "model".to_string(),
                        id: call_id.clone(),
                        started_at: progress.received_at,
                    },
                );
            }
            AgentLifecycleEventV1::ModelFinished { call_id, .. } => {
                self.active_operations
                    .remove(&OperationId(format!("{scope}:model:{call_id}")));
            }
            AgentLifecycleEventV1::ToolStarted { call_id, name } => {
                let id = OperationId(format!("{scope}:tool:{call_id}"));
                self.active_operations.insert(
                    id,
                    ActiveOperation {
                        scope: scope.clone(),
                        kind: OperationKind::Tool,
                        name: name.clone(),
                        id: call_id.clone(),
                        started_at: progress.received_at,
                    },
                );
            }
            AgentLifecycleEventV1::ToolFinished { call_id, .. } => {
                self.active_operations
                    .remove(&OperationId(format!("{scope}:tool:{call_id}")));
            }
            AgentLifecycleEventV1::AgentFinished { .. } => self.active_operations.clear(),
            AgentLifecycleEventV1::ModelProgress { .. }
            | AgentLifecycleEventV1::ModelRetrying { .. }
            | AgentLifecycleEventV1::SteeringApplied => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchdogTimerKind {
    NoProgress,
    MaxRun,
    CancellationGrace,
    ForcedTerminationGrace,
}

impl WorkerMachine {
    pub(super) fn no_progress_timer(&self, job_id: &str, now: EngineTime) -> WorkerRequest {
        let state = self.jobs.get(job_id).expect("timer job exists");
        let deadline = state.last_agent_progress + self.params.liveness_limits.max_no_progress;
        WorkerRequest::ArmWatchdogTimer {
            job_id: job_id.to_string(),
            attempt_id: state.attempt_id.clone(),
            generation: state.generation,
            timer_generation: state.timer_generation,
            kind: WatchdogTimerKind::NoProgress,
            delay: delay_until(now, deadline),
        }
    }

    pub(super) fn heartbeat_request(&self, now: EngineTime) -> WorkerRequest {
        WorkerRequest::SendHeartbeat(crate::client::heartbeat_message_params_reports(
            &self.params,
            self.jobs
                .iter()
                .map(|(job_id, state)| state.heartbeat(job_id, now))
                .collect(),
            self.free_capacity,
        ))
    }

    pub(super) fn begin_timeout(
        &mut self,
        now: EngineTime,
        job_id: String,
        reason: TimeoutReason,
        limit: Duration,
    ) -> Vec<WorkerRequest> {
        let Some(state) = self.jobs.get_mut(&job_id) else {
            return Vec::new();
        };
        if state.phase != JobPhase::Running {
            return Vec::new();
        }
        state.phase = JobPhase::CancelRequested;
        state.cancellation = CancellationStatus::Requested;
        let operation = state.current_operation();
        state.timeout = Some(TimeoutState {
            reason,
            limit,
            operation: operation.clone(),
        });
        let attempt_id = state.attempt_id.clone();
        let generation = state.generation;
        let run_elapsed_ms = elapsed_ms(now, state.run_started_at);
        let no_progress_elapsed_ms = elapsed_ms(now, state.last_agent_progress);
        let active_parallel_operation_count =
            u32::try_from(state.active_operations.len()).unwrap_or(u32::MAX);
        let reason_text = format!(
            "worker watchdog {} timeout after {}ms",
            reason.as_str(),
            limit.as_millis()
        );
        vec![
            WorkerRequest::Observe(crate::observability::WorkerEvent::JobTimeout {
                worker_id: self.params.worker_id.clone(),
                job_id: job_id.clone(),
                attempt_id: attempt_id.clone(),
                phase: "cancel_requested",
                reason: reason.as_str(),
                limit_ms: duration_ms(limit),
                run_elapsed_ms,
                last_progress_elapsed_ms: no_progress_elapsed_ms,
                no_progress_elapsed_ms,
                active_parallel_operation_count,
                operation: operation.as_ref().map(|operation| {
                    crate::observability::ObservedOperation::from_active(operation, now.as_nanos())
                }),
            }),
            WorkerRequest::Observe(crate::observability::WorkerEvent::CancellationRequested {
                worker_id: self.params.worker_id.clone(),
                job_id: job_id.clone(),
                attempt_id: attempt_id.clone(),
                reason: reason.as_str(),
                limit_ms: duration_ms(limit),
            }),
            self.heartbeat_request(now),
            WorkerRequest::CancelJob {
                job_id: job_id.clone(),
                attempt_id: attempt_id.clone(),
                generation,
                reason: reason_text,
            },
            WorkerRequest::ArmWatchdogTimer {
                job_id,
                attempt_id,
                generation,
                timer_generation: 0,
                kind: WatchdogTimerKind::CancellationGrace,
                delay: self.params.liveness_limits.graceful_cancellation_grace,
            },
        ]
    }

    pub(super) fn on_progress(
        &mut self,
        now: EngineTime,
        job_id: String,
        attempt_id: String,
        generation: u64,
        mut progress: JobProgress,
    ) -> Vec<WorkerRequest> {
        let Some(state) = self.jobs.get_mut(&job_id) else {
            return Vec::new();
        };
        if state.phase != JobPhase::Running
            || !state.accepts(&attempt_id, generation)
            || progress.attempt_id != attempt_id
            || progress.frame.validate().is_err()
            || progress.frame.seq <= state.last_progress_sequence
        {
            return Vec::new();
        }
        // The engine delivery stamp is the correctness clock. Child/source
        // timestamps and reporter-thread clocks are deliberately ignored.
        progress.received_at = now;
        state.last_progress_sequence = progress.frame.seq;
        state.last_agent_progress = now;
        state.timer_generation = state.timer_generation.saturating_add(1);
        state.update_operations(&progress);
        let operation = state.current_operation();
        let run_elapsed_ms = elapsed_ms(now, state.run_started_at);
        let active_parallel_operation_count =
            u32::try_from(state.active_operations.len()).unwrap_or(u32::MAX);
        vec![
            WorkerRequest::Observe(crate::observability::WorkerEvent::JobProgress {
                worker_id: self.params.worker_id.clone(),
                job_id: job_id.clone(),
                attempt_id,
                phase: "running",
                run_elapsed_ms,
                last_progress_elapsed_ms: 0,
                no_progress_elapsed_ms: 0,
                active_parallel_operation_count,
                operation: operation.as_ref().map(|operation| {
                    crate::observability::ObservedOperation::from_active(operation, now.as_nanos())
                }),
            }),
            self.no_progress_timer(&job_id, now),
        ]
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn on_watchdog_timer(
        &mut self,
        now: EngineTime,
        job_id: String,
        attempt_id: String,
        generation: u64,
        timer_generation: u64,
        kind: WatchdogTimerKind,
    ) -> Vec<WorkerRequest> {
        let Some(state) = self.jobs.get(&job_id) else {
            return Vec::new();
        };
        if !state.accepts(&attempt_id, generation) {
            return Vec::new();
        }
        match kind {
            WatchdogTimerKind::NoProgress => {
                if state.phase != JobPhase::Running || state.timer_generation != timer_generation {
                    return Vec::new();
                }
                let deadline =
                    state.last_agent_progress + self.params.liveness_limits.max_no_progress;
                if now <= deadline {
                    return vec![self.no_progress_timer(&job_id, now)];
                }
                self.begin_timeout(
                    now,
                    job_id,
                    TimeoutReason::NoProgress,
                    self.params.liveness_limits.max_no_progress,
                )
            }
            WatchdogTimerKind::MaxRun => {
                if state.phase != JobPhase::Running {
                    return Vec::new();
                }
                let Some(limit) = self.params.liveness_limits.max_run else {
                    return Vec::new();
                };
                let deadline = state.run_started_at + limit;
                if now <= deadline {
                    return vec![WorkerRequest::ArmWatchdogTimer {
                        job_id,
                        attempt_id,
                        generation,
                        timer_generation: 0,
                        kind,
                        delay: delay_until(now, deadline),
                    }];
                }
                self.begin_timeout(now, job_id, TimeoutReason::MaxRun, limit)
            }
            WatchdogTimerKind::CancellationGrace => {
                if state.phase != JobPhase::CancelRequested
                    || state.cancellation != CancellationStatus::Requested
                {
                    return Vec::new();
                }
                let state = self.jobs.get_mut(&job_id).expect("checked job exists");
                state.cancellation = CancellationStatus::Escalated;
                state.escalation_requested = true;
                vec![
                    WorkerRequest::EscalateJob {
                        job_id: job_id.clone(),
                        attempt_id: attempt_id.clone(),
                        generation,
                        hard: false,
                    },
                    WorkerRequest::ArmWatchdogTimer {
                        job_id,
                        attempt_id,
                        generation,
                        timer_generation: 0,
                        kind: WatchdogTimerKind::ForcedTerminationGrace,
                        delay: self.params.liveness_limits.forced_termination_grace,
                    },
                ]
            }
            WatchdogTimerKind::ForcedTerminationGrace => {
                if state.phase != JobPhase::CancelRequested {
                    return Vec::new();
                }
                vec![WorkerRequest::EscalateJob {
                    job_id,
                    attempt_id,
                    generation,
                    hard: true,
                }]
            }
        }
    }

    pub(super) fn record_terminal(
        &mut self,
        job_id: &str,
        attempt_id: &str,
        generation: u64,
        result: JobResult,
    ) -> Vec<WorkerRequest> {
        let Some(state) = self.jobs.get_mut(job_id) else {
            return Vec::new();
        };
        if !state.accepts(attempt_id, generation)
            || state.phase != JobPhase::Quiesced
            || state.pending_result.is_some()
        {
            return Vec::new();
        }
        state.result_durability = ResultDurabilityStatus::Pending;
        state.pending_result = Some(result.clone());
        vec![WorkerRequest::RecordResult {
            job_id: job_id.to_string(),
            attempt_id: attempt_id.to_string(),
            generation,
            result,
        }]
    }

    pub(super) fn timeout_result(
        &self,
        job_id: &str,
        state: &JobWatchState,
        now: EngineTime,
        cleanup: &JobCleanup,
    ) -> JobResult {
        let timeout = state
            .timeout
            .as_ref()
            .expect("cancelled quiescence carries timeout metadata");
        let operation = timeout.operation.as_ref().map(|operation| {
            serde_json::json!({
                "scope": operation.scope,
                "kind": match operation.kind { OperationKind::Model => "model", OperationKind::Tool => "tool" },
                "name": operation.name,
                "id": operation.id,
                "started_at_ms": operation.started_at.as_millis(),
                "elapsed_ms": now.as_nanos().saturating_sub(operation.started_at.as_nanos()) / 1_000_000,
            })
        });
        let cancellation = cleanup.cancellation.as_str();
        let descendant_cleanup = cleanup.descendants.as_str();
        let descendant_cleanup_error = cleanup.descendants.failure();
        let details = serde_json::json!({
            "timeout": {
                "reason": timeout.reason.as_str(),
                "limit_ms": u64::try_from(timeout.limit.as_millis()).unwrap_or(u64::MAX),
                "run_started_at_ms": state.run_started_at.as_millis(),
                "last_agent_progress_ms": state.last_agent_progress.as_millis(),
                "operation": operation,
                "cleanup": {
                    "cancellation": cancellation,
                    "descendants": descendant_cleanup,
                    "descendant_error": descendant_cleanup_error,
                    "escalated": state.escalation_requested,
                    "quiesced": true,
                }
            }
        });
        let operation_name = timeout
            .operation
            .as_ref()
            .map(|operation| operation.name.as_str())
            .unwrap_or("none");
        JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: self.params.worker_id.clone(),
            job_id: job_id.to_string(),
            attempt_id: Some(state.attempt_id.clone()),
            status: ResultStatus::Failure,
            repos: Vec::new(),
            verdict: None,
            title: None,
            body: None,
            children: Vec::new(),
            failure: Some(Failure {
                class: FailureClass::Transient,
                message: format!(
                    "worker watchdog timeout reason={} limit_ms={} operation={} cancellation={} descendants={}",
                    timeout.reason.as_str(),
                    timeout.limit.as_millis(),
                    operation_name,
                    cancellation,
                    descendant_cleanup,
                ),
            }),
            summary: None,
            details: Some(details),
        }
    }
}

fn elapsed_ms(now: EngineTime, started: EngineTime) -> u64 {
    now.as_nanos().saturating_sub(started.as_nanos()) / 1_000_000
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn delay_until(now: EngineTime, deadline: EngineTime) -> Duration {
    if now >= deadline {
        MIN_TIMER_TICK
    } else {
        Duration::from_nanos(deadline.as_nanos().saturating_sub(now.as_nanos()))
    }
}
