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
use temper_protocol_worker::{
    Artifact, JobResult, Poll, PullRequestFreshness, WorkerAuth, WorkerProtocolMessage,
};
#[cfg(test)]
use temper_worker_registry::daemon_core::QueuedJob;
use temper_worker_registry::{DaemonCore, WorkerPoolAuthConfig, WorkerPoolPolicy};

use crate::DEFAULT_MAX_POLL_WAIT_MS;
use crate::InFlightJob;
use crate::webhook::WebhookConfig;

/// `<io-event-completion>`s observed by the daemon machine.
pub(super) enum DaemonCompletion {
    /// One inbound HTTP request (worker protocol or webhook).
    Http {
        request: HttpRequestData,
        responder: HttpResponder,
    },
    /// A long-poll waiter's max-wait deadline elapsed.
    PollDeadline { id: u64 },
    /// A result applier finished off-loop.
    ApplyFinished { job_id: String },
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
    /// Daemon API: answer whether a correlation key still has pending or
    /// in-flight work known to the dispatch core.
    WorkstreamActive {
        correlation_key: String,
        reply: temper_engine_io::OneshotSender<bool>,
    },
    /// A webhook or companion change-source wake scan completed.
    WakeScanFinished { token: u64 },
    /// Daemon API: submit one lossy backend change hint to the wake-scan path.
    ChangeHint { hint: temper_runner::ChangeHint },
    /// Adjust the post-apply re-enqueue grace window.
    SetApplyGrace { apply_grace: Duration },
    /// Enable webhook intake with the given verification config.
    ConfigureWebhook { config: WebhookConfig },
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
    StartPollTimer {
        id: u64,
        delay: Duration,
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
    RunClaimAndRespond {
        job: InFlightJob,
        responder: HttpResponder,
        response: HttpResponseData,
    },
    RunPullRequestFreshnessCheck {
        check: PullRequestFreshness,
        responder: HttpResponder,
    },
    RunWakeScan {
        token: u64,
        hint: temper_runner::ChangeHint,
    },
    /// A role is at its concurrency limit with same-role work queued behind it.
    /// Carries the per-role concurrency figure and the `artifact.ref` strings of
    /// the waiting items, ready for the §7 `worker` / `role.saturated` line.
    RoleSaturated {
        role: String,
        concurrency: u64,
        waiting: Vec<String>,
    },
    Log(String),
    WorkstreamActiveReply(temper_engine_io::OneshotSender<bool>, bool),
    #[cfg(test)]
    QueuedJobsReply(
        temper_engine_io::OneshotSender<Vec<QueuedJob>>,
        Vec<QueuedJob>,
    ),
}

pub(super) struct PollWaiter {
    pub(super) poll: Poll,
    pub(super) auth: Option<WorkerAuth>,
    pub(super) responder: HttpResponder,
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
    pub(super) webhook_waiters: BTreeMap<u64, HttpResponder>,
    pub(super) applying: BTreeSet<String>,
    pub(super) recently_applied: BTreeMap<String, EngineTime>,
    pub(super) apply_grace: Duration,
    /// The engine's once-per-delivery clock snapshot; updated as the first
    /// act of every transition, before any handler logic runs.
    pub(super) now: EngineTime,
    pub(super) next_id: u64,
}

impl DaemonMachine {
    pub(super) fn new(apply_grace: Duration, max_poll_wait_ms: u64) -> Self {
        Self::with_core(DaemonCore::new(), apply_grace, max_poll_wait_ms)
    }

    pub(super) fn with_worker_pools(
        worker_pools: Vec<WorkerPoolPolicy>,
        apply_grace: Duration,
        max_poll_wait_ms: u64,
    ) -> Self {
        Self::with_core(
            DaemonCore::with_pool_policies(worker_pools),
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
            webhook_waiters: BTreeMap::new(),
            applying: BTreeSet::new(),
            recently_applied: BTreeMap::new(),
            apply_grace,
            now: EngineTime::ZERO,
            next_id: 0,
        }
    }

    pub(super) fn default_machine(apply_grace: Duration) -> Self {
        Self::new(apply_grace, DEFAULT_MAX_POLL_WAIT_MS)
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

    pub(super) fn next_token(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
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
                let Some(waiter) = self.waiters.remove(&id) else {
                    return Vec::new();
                };
                let response = match self.core.handle_authenticated(
                    WorkerProtocolMessage::Poll(waiter.poll.clone()),
                    waiter.auth.as_ref(),
                ) {
                    Ok(response) => response.expect("poll messages produce a response"),
                    Err(_) => {
                        return vec![DaemonRequest::Respond {
                            responder: waiter.responder,
                            response: HttpResponseData::status_only(401),
                        }];
                    }
                };
                self.poll_response_requests(response, &waiter.poll.worker_id, waiter.responder)
            }
            DaemonCompletion::ApplyFinished { job_id } => {
                self.applying.remove(&job_id);
                self.recently_applied
                    .insert(job_id, self.now + self.apply_grace);
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
            DaemonCompletion::WorkstreamActive {
                correlation_key,
                reply,
            } => vec![DaemonRequest::WorkstreamActiveReply(
                reply,
                self.core
                    .workstream_active_by_correlation_key(&correlation_key),
            )],
            DaemonCompletion::WakeScanFinished { token } => {
                match self.webhook_waiters.remove(&token) {
                    Some(responder) => vec![DaemonRequest::Respond {
                        responder,
                        response: HttpResponseData::status_only(202),
                    }],
                    None => Vec::new(),
                }
            }
            DaemonCompletion::ChangeHint { hint } => {
                let token = self.next_token();
                vec![DaemonRequest::RunWakeScan { token, hint }]
            }
            DaemonCompletion::SetApplyGrace { apply_grace } => {
                self.apply_grace = apply_grace;
                Vec::new()
            }
            DaemonCompletion::ConfigureWebhook { config } => {
                self.webhook = Some(config);
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
}
