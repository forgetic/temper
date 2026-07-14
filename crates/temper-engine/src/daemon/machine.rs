// SPDX-License-Identifier: MPL-2.0

//! The daemon's functional core: the deterministic state machine, the
//! completions it observes, and the requests it issues. The per-message
//! handler logic lives in [`super::handlers`]; the imperative shell that
//! performs requests lives in [`super::executor`].

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use temper_engine_io::http::{HttpRequestData, HttpResponder, HttpResponseData};
use temper_engine_io::{EngineTime, Machine};
use temper_forge::RepositoryPath;
use temper_protocol_worker::{
    Artifact, Assign, ContextResponse, ErrorCode, FetchContext, JobResult, Poll, ProtocolError,
    PullRequestFreshness, WORKER_PROTOCOL_VERSION, WorkerAuth, WorkerProtocolMessage,
};
#[cfg(test)]
use temper_worker_registry::daemon_core::QueuedJob;
use temper_worker_registry::{
    DaemonCore, RecoveredJob, RegistryError, WorkerPoolAuthConfig, WorkerPoolPolicy,
};

use crate::DEFAULT_MAX_POLL_WAIT_MS;
use crate::InFlightJob;
use crate::applier::{ApplyOutcome, ClaimOutcome};
use crate::webhook::WebhookConfig;

use super::wake_coordinator::{
    BroadMode, WakeCoordinator, WakeLane, WakeOutcome, WakeRequest, WakeWork,
};
pub(super) use super::wake_observability::WakeMeasurement;

/// `<io-event-completion>`s observed by the daemon machine.
pub(super) enum DaemonCompletion {
    /// One inbound HTTP request (worker protocol or webhook).
    Http {
        request: HttpRequestData,
        responder: HttpResponder,
    },
    /// A long-poll waiter's max-wait deadline elapsed.
    PollDeadline { id: u64 },
    /// A durable assignment claim finished off-loop.
    ClaimFinished {
        assign: Assign,
        worker_id: String,
        responder: HttpResponder,
        outcome: ClaimOutcome,
    },
    AssignmentDeliveryFailed {
        job: InFlightJob,
        context: crate::applier::ClaimContext,
    },
    /// A result applier finished off-loop.
    ApplyFinished {
        job_id: String,
        outcome: ApplyOutcome,
    },
    /// Close the dispatch barrier before startup inventory begins.
    BeginStartupRecovery,
    /// Stage one durable prior-boot assignment for heartbeat reattachment.
    StageRecoveredJob {
        job: RecoveredJob,
        daemon_boot_id: String,
        reply: temper_engine_io::OneshotSender<Result<(), RegistryError>>,
    },
    /// Detach claims that received no matching heartbeat while keeping the
    /// dispatch barrier closed for Forge convergence.
    CollectStartupOrphans {
        reply: temper_engine_io::OneshotSender<Vec<RecoveredJob>>,
    },
    /// Open the barrier only after every detached claim converged in Forge.
    CompleteStartupRecovery {
        reply: temper_engine_io::OneshotSender<()>,
    },
    ArmStartupRecoveryGrace {
        delay: Duration,
        reply: temper_engine_io::OneshotSender<()>,
    },
    StartupRecoveryGraceElapsed {
        reply: temper_engine_io::OneshotSender<()>,
    },
    ReleaseAssignmentsForShutdown {
        reply: temper_engine_io::OneshotSender<()>,
    },
    /// Stop the daemon loop without releasing durable assignments.
    Crash {
        reply: temper_engine_io::OneshotSender<()>,
    },
    /// Daemon API: enqueue one job (scans, backstops, tests).
    Enqueue {
        job_id: String,
        role: String,
        repo: String,
        artifact: Artifact,
        job_payload: serde_json::Value,
    },
    /// Daemon API: reconcile stale pending jobs after a successful role scan.
    ReconcilePendingRoleJobs {
        repo: String,
        role: String,
        current_job_ids: BTreeSet<String>,
    },
    /// Reconcile stale pending jobs only for one `(repo, role, artifact)`
    /// targeted view.
    ReconcilePendingTargetedRoleJobs {
        repo: String,
        role: String,
        artifact: Artifact,
        current_job_ids: BTreeSet<String>,
    },
    /// Daemon API: answer whether a correlation key still has pending or
    /// in-flight work known to the dispatch core.
    WorkstreamActive {
        correlation_key: String,
        reply: temper_engine_io::OneshotSender<bool>,
    },
    /// Schedule one bounded wake request after webhook verification or from a
    /// companion/startup source.
    ScheduleWake { request: WakeRequest },
    /// One generation-tagged leading-edge debounce timer elapsed.
    WakeTimerElapsed {
        repo: RepositoryPath,
        generation: u64,
    },
    /// One admitted repository wake finished.
    WakeFinished {
        work: WakeWork,
        outcome: WakeOutcome,
    },
    /// Install configured repository/lane routes before accepting wake hints.
    ConfigureWakeRepositories {
        repositories: Vec<(RepositoryPath, BTreeSet<WakeLane>)>,
        unresolved_lanes: BTreeSet<WakeLane>,
        configured_repository_limit: usize,
    },
    /// Adjust the leading-edge debounce and global repository-run cap.
    ConfigureWakeScheduling {
        debounce: Duration,
        max_in_flight_repositories: usize,
    },
    /// Adjust the post-apply re-enqueue grace window.
    SetApplyGrace { apply_grace: Duration },
    /// Enable webhook intake with the given verification config.
    ConfigureWebhook { config: WebhookConfig },
    /// Installs the immutable repository authorization catalog for context reads.
    ConfigureArtifactContextCatalog {
        catalog: crate::ConfiguredRepositoryCatalog,
    },
    /// Enable worker-pool authentication with the given pool/token policy.
    ConfigureWorkerPoolAuth { config: WorkerPoolAuthConfig },
    #[cfg(test)]
    QueuedJobs {
        reply: temper_engine_io::OneshotSender<Vec<QueuedJob>>,
    },
}

/// `<io-event-request>`s the daemon machine may issue.
pub(super) enum DaemonRequest {
    Respond {
        responder: HttpResponder,
        response: HttpResponseData,
    },
    RespondAssignment {
        responder: HttpResponder,
        response: HttpResponseData,
        job: InFlightJob,
        context: crate::applier::ClaimContext,
    },
    StartPollTimer {
        id: u64,
        delay: Duration,
    },
    StartStartupRecoveryGrace {
        delay: Duration,
        reply: temper_engine_io::OneshotSender<()>,
    },
    RunApply {
        job: InFlightJob,
        result: JobResult,
    },
    /// Apply retry bookkeeping before acknowledging a retryable/canceled result
    /// to the worker so the source claim is released before the next rescan.
    RunApplyAndRespond {
        job: InFlightJob,
        result: JobResult,
        responder: HttpResponder,
        response: HttpResponseData,
    },
    /// Apply an assignment-time source claim before returning the assignment to
    /// the worker that will start the job.
    RunClaim {
        job: InFlightJob,
        worker_id: String,
        daemon_boot_id: String,
        assign: Assign,
        responder: HttpResponder,
    },
    RunClaimRollback {
        job: InFlightJob,
        context: crate::applier::ClaimContext,
    },
    /// Refresh exact recovered assignments before acknowledging their heartbeat.
    RunHeartbeatsAndRespond {
        assignments: Vec<(InFlightJob, crate::applier::ClaimContext)>,
        responder: HttpResponder,
        response: HttpResponseData,
    },
    RunShutdownRelease {
        assignments: Vec<(InFlightJob, crate::applier::ClaimContext)>,
        reply: temper_engine_io::OneshotSender<()>,
    },
    RunPullRequestFreshnessCheck {
        check: PullRequestFreshness,
        responder: HttpResponder,
    },
    RunFetchContext {
        request: FetchContext,
        role: String,
        responder: HttpResponder,
    },
    RespondContext {
        response: ContextResponse,
        audit: ContextReadAudit,
        responder: HttpResponder,
    },
    StartWakeTimer {
        repo: RepositoryPath,
        generation: u64,
        delay: Duration,
    },
    RunWake {
        work: WakeWork,
    },
    /// A role is at its configured concurrency limit with same-role work
    /// pending. Carries the configured figure and the `artifact.ref` strings of
    /// the waiting items, ready for the §7 `worker` / `role.saturated` line.
    RoleSaturated {
        role: String,
        concurrency: u64,
        waiting: Vec<String>,
    },
    WakeMeasurement(WakeMeasurement),
    Log(String),
    WorkstreamActiveReply(temper_engine_io::OneshotSender<bool>, bool),
    #[cfg(test)]
    QueuedJobsReply(
        temper_engine_io::OneshotSender<Vec<QueuedJob>>,
        Vec<QueuedJob>,
    ),
}

pub(super) struct ContextReadAudit {
    pub(super) worker_id: String,
    pub(super) job_id: String,
    pub(super) role: String,
    pub(super) operation: String,
    pub(super) repository: String,
    pub(super) item_number: u64,
    pub(super) status: String,
}

pub(super) struct PollWaiter {
    pub(super) poll: Poll,
    pub(super) auth: Option<WorkerAuth>,
    pub(super) responder: HttpResponder,
}

pub(super) struct DeferredEnqueue {
    pub(super) job_id: String,
    pub(super) role: String,
    pub(super) repo: String,
    pub(super) artifact: Artifact,
    pub(super) job_payload: serde_json::Value,
}

/// The daemon's functional core: deterministic worker-protocol, long-poll,
/// apply-window, and webhook-verification logic. No I/O, no clocks — time
/// arrives as data on completions; everything it wants done leaves as
/// [`DaemonRequest`] values.
pub(super) struct DaemonMachine {
    pub(super) core: DaemonCore,
    pub(super) max_poll_wait_ms: u64,
    pub(super) webhook: Option<WebhookConfig>,
    pub(super) waiters: BTreeMap<u64, PollWaiter>,
    pub(super) applying: BTreeSet<String>,
    pub(super) recently_applied: BTreeMap<String, EngineTime>,
    pub(super) retry_attempts: BTreeMap<String, u32>,
    pub(super) retry_backoff_until: BTreeMap<String, EngineTime>,
    pub(super) apply_grace: Duration,
    /// The engine's once-per-delivery clock snapshot; updated as the first
    /// act of every transition, before any handler logic runs.
    pub(super) now: EngineTime,
    pub(super) daemon_boot_id: String,
    /// Closed while durable assignments are inventoried and offered a bounded
    /// heartbeat reattachment grace period.
    pub(super) startup_recovery: bool,
    pub(super) deferred_enqueues: Vec<DeferredEnqueue>,
    pub(super) assignment_contexts: BTreeMap<String, crate::applier::ClaimContext>,
    pub(super) artifact_catalog: crate::ConfiguredRepositoryCatalog,
    pub(super) wake_coordinator: WakeCoordinator,
    pub(super) next_id: u64,
    pub(super) stopped: bool,
}

impl DaemonMachine {
    pub(super) fn new(apply_grace: Duration, max_poll_wait_ms: u64) -> Self {
        Self::with_core(DaemonCore::new(), apply_grace, max_poll_wait_ms)
    }

    pub(super) fn with_role_limits(
        role_limits: BTreeMap<String, u32>,
        apply_grace: Duration,
        max_poll_wait_ms: u64,
    ) -> Self {
        Self::with_core(
            DaemonCore::with_role_limits(role_limits),
            apply_grace,
            max_poll_wait_ms,
        )
    }

    pub(super) fn with_worker_pools(
        worker_pools: Vec<WorkerPoolPolicy>,
        apply_grace: Duration,
        max_poll_wait_ms: u64,
    ) -> Self {
        Self::with_worker_pools_and_role_limits(
            worker_pools,
            BTreeMap::new(),
            apply_grace,
            max_poll_wait_ms,
        )
    }

    pub(super) fn with_worker_pools_and_role_limits(
        worker_pools: Vec<WorkerPoolPolicy>,
        role_limits: BTreeMap<String, u32>,
        apply_grace: Duration,
        max_poll_wait_ms: u64,
    ) -> Self {
        Self::with_core(
            DaemonCore::with_pool_policies_and_role_limits(worker_pools, role_limits),
            apply_grace,
            max_poll_wait_ms,
        )
    }

    fn with_core(core: DaemonCore, apply_grace: Duration, max_poll_wait_ms: u64) -> Self {
        Self {
            core,
            max_poll_wait_ms,
            webhook: None,
            waiters: BTreeMap::new(),
            applying: BTreeSet::new(),
            recently_applied: BTreeMap::new(),
            retry_attempts: BTreeMap::new(),
            retry_backoff_until: BTreeMap::new(),
            apply_grace,
            now: EngineTime::ZERO,
            daemon_boot_id: new_daemon_boot_id(),
            startup_recovery: false,
            deferred_enqueues: Vec::new(),
            assignment_contexts: BTreeMap::new(),
            artifact_catalog: crate::ConfiguredRepositoryCatalog::default(),
            wake_coordinator: WakeCoordinator::default(),
            next_id: 0,
            stopped: false,
        }
    }

    pub(super) fn default_machine(apply_grace: Duration) -> Self {
        Self::new(apply_grace, DEFAULT_MAX_POLL_WAIT_MS)
    }

    pub(super) fn default_machine_with_role_limits(
        apply_grace: Duration,
        role_limits: BTreeMap<String, u32>,
    ) -> Self {
        if role_limits.is_empty() {
            Self::default_machine(apply_grace)
        } else {
            Self::with_role_limits(role_limits, apply_grace, DEFAULT_MAX_POLL_WAIT_MS)
        }
    }

    pub(super) fn default_machine_with_worker_pools(
        apply_grace: Duration,
        worker_pools: Vec<WorkerPoolPolicy>,
    ) -> Self {
        if worker_pools.is_empty() {
            Self::default_machine(apply_grace)
        } else {
            Self::with_worker_pools(worker_pools, apply_grace, DEFAULT_MAX_POLL_WAIT_MS)
        }
    }

    pub(super) fn default_machine_with_worker_pools_and_role_limits(
        apply_grace: Duration,
        worker_pools: Vec<WorkerPoolPolicy>,
        role_limits: BTreeMap<String, u32>,
    ) -> Self {
        if worker_pools.is_empty() {
            return Self::default_machine_with_role_limits(apply_grace, role_limits);
        }
        if role_limits.is_empty() {
            return Self::default_machine_with_worker_pools(apply_grace, worker_pools);
        }
        Self::with_worker_pools_and_role_limits(
            worker_pools,
            role_limits,
            apply_grace,
            DEFAULT_MAX_POLL_WAIT_MS,
        )
    }

    pub(super) fn next_token(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    pub(super) fn schedule_wake(&mut self, request: WakeRequest) -> Vec<DaemonRequest> {
        let apply_active = !self.applying.is_empty();
        let decisions =
            if request.lanes.is_empty() && request.scope.broad_mode() == Some(BroadMode::Startup) {
                self.wake_coordinator
                    .schedule_startup_broad(self.now, request.repo, apply_active)
            } else {
                self.wake_coordinator
                    .schedule(self.now, request, apply_active)
            };
        self.wake_decision_requests(decisions)
    }
}

fn in_flight_job_from_assignment(assign: &Assign) -> InFlightJob {
    InFlightJob {
        job_id: assign.job_id.clone(),
        role: assign.role.clone(),
        repo: assign.repo.clone(),
        artifact: assign.artifact.clone(),
        job_payload: assign.job_payload.clone(),
    }
}

fn claim_failure_response(
    responder: HttpResponder,
    job_id: String,
    reason: String,
) -> DaemonRequest {
    DaemonRequest::Respond {
        responder,
        response: super::protocol::protocol_response(Some(WorkerProtocolMessage::Error(
            ProtocolError {
                protocol_version: WORKER_PROTOCOL_VERSION,
                code: ErrorCode::PollTimeout,
                message: format!("durable assignment claim failed: {reason}"),
                retry_after_ms: Some(100),
                job_id: Some(job_id),
            },
        ))),
    }
}

fn new_daemon_boot_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_BOOT: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT_BOOT.fetch_add(1, Ordering::Relaxed);
    let epoch_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("daemon-boot-{epoch_nanos:x}-{sequence:x}")
}

fn retry_delay(attempt: u32) -> Duration {
    const MAX_BACKOFF_SECS: u64 = 300;
    let exponent = attempt.saturating_sub(1).min(8);
    Duration::from_secs((2_u64.saturating_pow(exponent)).min(MAX_BACKOFF_SECS))
}

impl Machine for DaemonMachine {
    type Completion = DaemonCompletion;
    type Request = DaemonRequest;

    fn on_completion(
        &mut self,
        now: EngineTime,
        completion: DaemonCompletion,
    ) -> Vec<DaemonRequest> {
        self.now = now;
        match completion {
            DaemonCompletion::Http { request, responder } => self.handle_http(request, responder),
            DaemonCompletion::PollDeadline { id } => {
                if self.startup_recovery {
                    return Vec::new();
                }
                let Some(waiter) = self.waiters.remove(&id) else {
                    return Vec::new();
                };
                let response = match self
                    .core
                    .reserve_authenticated_poll(waiter.poll.clone(), waiter.auth.as_ref())
                {
                    Ok(response) => response,
                    Err(_) => {
                        return vec![DaemonRequest::Respond {
                            responder: waiter.responder,
                            response: HttpResponseData::status_only(401),
                        }];
                    }
                };
                self.poll_response_requests(response, &waiter.poll.worker_id, waiter.responder)
            }
            DaemonCompletion::ClaimFinished {
                assign,
                worker_id,
                responder,
                outcome,
            } => {
                let context = crate::applier::ClaimContext {
                    worker_id: worker_id.clone(),
                    daemon_boot_id: self.daemon_boot_id.clone(),
                };
                if !responder.is_open() {
                    self.core.rollback_assignment(&assign.job_id);
                    return vec![DaemonRequest::RunClaimRollback {
                        job: in_flight_job_from_assignment(&assign),
                        context,
                    }];
                }
                match outcome {
                    ClaimOutcome::Claimed => {
                        if self.core.commit_assignment(&assign.job_id).is_ok() {
                            self.assignment_contexts
                                .insert(assign.job_id.clone(), context.clone());
                            vec![
                                DaemonRequest::Log(super::protocol::assignment_log_line(
                                    &assign, &worker_id,
                                )),
                                DaemonRequest::RespondAssignment {
                                    job: in_flight_job_from_assignment(&assign),
                                    context,
                                    responder,
                                    response: super::protocol::protocol_response(Some(
                                        WorkerProtocolMessage::Assign(assign),
                                    )),
                                },
                            ]
                        } else {
                            self.core.rollback_assignment(&assign.job_id);
                            vec![claim_failure_response(
                                responder,
                                assign.job_id,
                                "assignment reservation became stale".to_string(),
                            )]
                        }
                    }
                    ClaimOutcome::Stale { reason } => {
                        self.core.discard_assignment_reservation(&assign.job_id);
                        vec![claim_failure_response(responder, assign.job_id, reason)]
                    }
                    ClaimOutcome::Contended { reason } | ClaimOutcome::Retryable { reason } => {
                        self.core.rollback_assignment(&assign.job_id);
                        vec![claim_failure_response(responder, assign.job_id, reason)]
                    }
                }
            }
            DaemonCompletion::AssignmentDeliveryFailed { job, context } => {
                self.assignment_contexts.remove(&job.job_id);
                self.core.rollback_committed_assignment(&job.job_id);
                vec![DaemonRequest::RunClaimRollback { job, context }]
            }
            DaemonCompletion::ApplyFinished { job_id, outcome } => {
                self.applying.remove(&job_id);
                let mut requests = match outcome {
                    ApplyOutcome::Retryable { reason } => {
                        let attempt = self.retry_attempts.entry(job_id.clone()).or_insert(0);
                        *attempt = attempt.saturating_add(1);
                        let delay = retry_delay(*attempt);
                        self.retry_backoff_until
                            .insert(job_id.clone(), self.now + delay);
                        vec![DaemonRequest::Log(format!(
                            "engine: result apply retry scheduled job_id={job_id} attempt={} backoff_ms={} reason={reason}",
                            *attempt,
                            delay.as_millis()
                        ))]
                    }
                    ApplyOutcome::Applied | ApplyOutcome::Stale | ApplyOutcome::Rejected { .. } => {
                        self.retry_attempts.remove(&job_id);
                        self.retry_backoff_until.remove(&job_id);
                        self.recently_applied
                            .insert(job_id, self.now + self.apply_grace);
                        Vec::new()
                    }
                };
                if self.applying.is_empty() {
                    let decisions = self.wake_coordinator.promote_apply_deferred();
                    requests.extend(self.wake_decision_requests(decisions));
                }
                requests
            }
            DaemonCompletion::BeginStartupRecovery => {
                self.startup_recovery = true;
                Vec::new()
            }
            DaemonCompletion::StageRecoveredJob {
                job,
                daemon_boot_id,
                reply,
            } => {
                let job_id = job.job_id.clone();
                let worker_id = job.worker_id.clone();
                let outcome = self.core.stage_recovered_job(job);
                if outcome.is_ok() {
                    self.assignment_contexts.insert(
                        job_id,
                        crate::applier::ClaimContext {
                            worker_id,
                            daemon_boot_id,
                        },
                    );
                }
                reply.send(outcome);
                Vec::new()
            }
            DaemonCompletion::CollectStartupOrphans { reply } => {
                let orphaned = self.core.take_unreattached_recovered_jobs();
                for job in &orphaned {
                    self.assignment_contexts.remove(&job.job_id);
                }
                reply.send(orphaned);
                Vec::new()
            }
            DaemonCompletion::CompleteStartupRecovery { reply } => {
                self.startup_recovery = false;
                let deferred = std::mem::take(&mut self.deferred_enqueues);
                let mut requests = Vec::new();
                for enqueue in deferred {
                    requests.extend(self.handle_enqueue(
                        enqueue.job_id,
                        enqueue.role,
                        enqueue.repo,
                        enqueue.artifact,
                        enqueue.job_payload,
                    ));
                }
                requests.extend(self.fulfil_waiters());
                for id in self.waiters.keys().copied() {
                    requests.push(DaemonRequest::StartPollTimer {
                        id,
                        delay: Duration::from_millis(self.max_poll_wait_ms),
                    });
                }
                // Coordinator state is intentionally volatile. Once durable
                // startup convergence opens the barrier, submit one broad
                // recovery generation per configured repository so lost
                // pending/dirty hints are never a correctness dependency.
                for repo in self.wake_coordinator.configured_repositories() {
                    requests
                        .extend(self.schedule_wake(WakeRequest::broad(repo, BroadMode::Startup)));
                }
                reply.send(());
                requests
            }
            DaemonCompletion::ArmStartupRecoveryGrace { delay, reply } => {
                vec![DaemonRequest::StartStartupRecoveryGrace { delay, reply }]
            }
            DaemonCompletion::StartupRecoveryGraceElapsed { reply } => {
                reply.send(());
                Vec::new()
            }
            DaemonCompletion::ReleaseAssignmentsForShutdown { reply } => {
                let jobs = self.core.in_flight_jobs();
                let mut assignments = Vec::new();
                for job in jobs {
                    if let Some(context) = self.assignment_contexts.remove(&job.job_id) {
                        self.core.coordinator_mut().complete(&job.job_id).ok();
                        assignments.push((job, context));
                    }
                }
                vec![DaemonRequest::RunShutdownRelease { assignments, reply }]
            }
            DaemonCompletion::Crash { reply } => {
                self.stopped = true;
                reply.send(());
                Vec::new()
            }
            DaemonCompletion::Enqueue {
                job_id,
                role,
                repo,
                artifact,
                job_payload,
            } => self.handle_enqueue(job_id, role, repo, artifact, job_payload),
            DaemonCompletion::ReconcilePendingRoleJobs {
                repo,
                role,
                current_job_ids,
            } => self.handle_reconcile_pending_role_jobs(repo, role, current_job_ids),
            DaemonCompletion::ReconcilePendingTargetedRoleJobs {
                repo,
                role,
                artifact,
                current_job_ids,
            } => self.handle_reconcile_pending_targeted_role_jobs(
                repo,
                role,
                artifact,
                current_job_ids,
            ),
            DaemonCompletion::WorkstreamActive {
                correlation_key,
                reply,
            } => vec![DaemonRequest::WorkstreamActiveReply(
                reply,
                self.core
                    .workstream_active_by_correlation_key(&correlation_key),
            )],
            DaemonCompletion::ScheduleWake { request } => self.schedule_wake(request),
            DaemonCompletion::WakeTimerElapsed { repo, generation } => {
                let decisions = self.wake_coordinator.timer_elapsed(
                    self.now,
                    repo,
                    generation,
                    !self.applying.is_empty(),
                );
                self.wake_decision_requests(decisions)
            }
            DaemonCompletion::WakeFinished { work, outcome } => {
                let decisions = self.wake_coordinator.finish(
                    self.now,
                    &work,
                    outcome,
                    !self.applying.is_empty(),
                );
                self.wake_decision_requests(decisions)
            }
            DaemonCompletion::ConfigureWakeRepositories {
                repositories,
                unresolved_lanes,
                configured_repository_limit,
            } => {
                for (repo, lanes) in repositories {
                    self.wake_coordinator.configure_repository(repo, lanes);
                }
                self.wake_coordinator.configure_unresolved_repositories(
                    unresolved_lanes,
                    configured_repository_limit,
                );
                Vec::new()
            }
            DaemonCompletion::ConfigureWakeScheduling {
                debounce,
                max_in_flight_repositories,
            } => {
                self.wake_coordinator
                    .configure(debounce, max_in_flight_repositories);
                Vec::new()
            }
            DaemonCompletion::SetApplyGrace { apply_grace } => {
                self.apply_grace = apply_grace;
                Vec::new()
            }
            DaemonCompletion::ConfigureWebhook { config } => {
                self.webhook = Some(config);
                Vec::new()
            }
            DaemonCompletion::ConfigureArtifactContextCatalog { catalog } => {
                self.artifact_catalog = catalog;
                Vec::new()
            }
            DaemonCompletion::ConfigureWorkerPoolAuth { config } => {
                self.core.configure_worker_pool_auth(config);
                Vec::new()
            }
            #[cfg(test)]
            DaemonCompletion::QueuedJobs { reply } => {
                vec![DaemonRequest::QueuedJobsReply(
                    reply,
                    self.core.queued_jobs(),
                )]
            }
        }
    }

    fn is_stopped(&self) -> bool {
        self.stopped
    }
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
