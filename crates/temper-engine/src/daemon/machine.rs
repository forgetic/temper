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
use temper_worker_protocol::{Artifact, JobProgress, JobResult, Poll, WorkerProtocolMessage};
use temper_worker_registry::DaemonCore;
#[cfg(test)]
use temper_worker_registry::daemon_core::QueuedJob;

use crate::DEFAULT_MAX_POLL_WAIT_MS;
use crate::InFlightJob;
use crate::webhook::WebhookConfig;

use super::protocol::protocol_response;

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
    /// A progress-checkpoint application finished off-loop (no state).
    ProgressApplyFinished,
    /// Daemon API: enqueue one job (scans, backstops, tests).
    Enqueue {
        job_id: String,
        role: String,
        repo: String,
        artifact: Artifact,
        job_payload: serde_json::Value,
    },
    /// A webhook wake scan completed; release the held `202` response.
    WakeScanFinished { token: u64 },
    /// Adjust the post-apply re-enqueue grace window.
    SetApplyGrace { apply_grace: Duration },
    /// Enable webhook intake with the given verification config.
    ConfigureWebhook { config: WebhookConfig },
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
    RunProgressApply {
        job: InFlightJob,
        progress: JobProgress,
    },
    RunWakeScan {
        token: u64,
        hint: temper_runner::ChangeHint,
    },
    Log(String),
    #[cfg(test)]
    QueuedJobsReply(
        temper_engine_io::OneshotSender<Vec<QueuedJob>>,
        Vec<QueuedJob>,
    ),
}

pub(super) struct PollWaiter {
    pub(super) poll: Poll,
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
        Self {
            core: DaemonCore::new(),
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
                let response = self
                    .core
                    .handle(WorkerProtocolMessage::Poll(waiter.poll.clone()))
                    .expect("poll messages produce a response");
                vec![DaemonRequest::Respond {
                    responder: waiter.responder,
                    response: protocol_response(Some(response)),
                }]
            }
            DaemonCompletion::ApplyFinished { job_id } => {
                self.applying.remove(&job_id);
                self.recently_applied
                    .insert(job_id, self.now + self.apply_grace);
                Vec::new()
            }
            // Progress application carries no machine state; the completion
            // exists only so the executor's async work feeds back per engine
            // discipline.
            DaemonCompletion::ProgressApplyFinished => Vec::new(),
            DaemonCompletion::Enqueue {
                job_id,
                role,
                repo,
                artifact,
                job_payload,
            } => self.handle_enqueue(job_id, role, repo, artifact, job_payload),
            DaemonCompletion::WakeScanFinished { token } => {
                match self.webhook_waiters.remove(&token) {
                    Some(responder) => vec![DaemonRequest::Respond {
                        responder,
                        response: HttpResponseData::status_only(202),
                    }],
                    None => Vec::new(),
                }
            }
            DaemonCompletion::SetApplyGrace { apply_grace } => {
                self.apply_grace = apply_grace;
                Vec::new()
            }
            DaemonCompletion::ConfigureWebhook { config } => {
                self.webhook = Some(config);
                Vec::new()
            }
            #[cfg(test)]
            DaemonCompletion::QueuedJobs { reply } => {
                vec![DaemonRequest::QueuedJobsReply(reply, self.core.queued_jobs())]
            }
        }
    }
}
