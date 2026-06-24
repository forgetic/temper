// SPDX-License-Identifier: MPL-2.0

//! Per-message handler logic for [`DaemonMachine`]: HTTP routing, worker
//! protocol dispatch, webhook delivery verification, enqueue gating, and
//! long-poll waiter fulfilment. Pure transitions returning [`DaemonRequest`]s.

use std::collections::BTreeMap;
use std::time::Duration;

use temper_engine_io::http::{HttpRequestData, HttpResponder, HttpResponseData};
use temper_log::{WorkItemRef, strip_provider_scheme};
use temper_protocol_worker::{Artifact, JobProgress, JobResult, Poll, WorkerProtocolMessage};

use crate::webhook::{WebhookError, parse_verified_webhook, webhook_accepted_log_line};

use super::machine::{DaemonMachine, DaemonRequest, PollWaiter};
use super::protocol::{
    ResultDisposition, assignment_log_line, is_poll_timeout, progress_log_line, protocol_response,
    result_disposition, result_disposition_log_value, result_received_log_line,
};
use super::state_dto::{DaemonStateSnapshot, JobDto};

impl DaemonMachine {
    pub(super) fn handle_http(
        &mut self,
        request: HttpRequestData,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        match (request.method.as_str(), request.uri.as_str()) {
            ("POST", "/v1/message") => self.handle_protocol_message(&request.body, responder),
            ("POST", "/forgejo/webhook") if self.webhook.is_some() => {
                self.handle_webhook_delivery(&request, responder)
            }
            ("GET", "/v1/state") => self.handle_state_snapshot(responder),
            ("GET", uri) if uri.starts_with("/v1/state/job/") => {
                let job_id = &uri["/v1/state/job/".len()..];
                self.handle_state_job(job_id, responder)
            }
            _ => vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(404),
            }],
        }
    }

    /// Answers `GET /v1/state` with the full raw daemon dispatch snapshot.
    ///
    /// A pure in-process read over the single-threaded daemon core — no new
    /// locking. The board (temper-web, PR D) projects this raw view into its
    /// card model; see [`DaemonStateSnapshot`].
    fn handle_state_snapshot(&self, responder: HttpResponder) -> Vec<DaemonRequest> {
        let snapshot = DaemonStateSnapshot::from_core(&self.core);
        vec![DaemonRequest::Respond {
            responder,
            response: HttpResponseData::json(200, &snapshot.to_json()),
        }]
    }

    /// Answers `GET /v1/state/job/{id}` with one in-flight job DTO, or 404 when
    /// the job is pending, unknown, or already completed.
    fn handle_state_job(&self, job_id: &str, responder: HttpResponder) -> Vec<DaemonRequest> {
        let response = match self.core.in_flight_job(job_id) {
            Some(job) => HttpResponseData::json(200, &JobDto::from(&job).to_json()),
            None => HttpResponseData::status_only(404),
        };
        vec![DaemonRequest::Respond {
            responder,
            response,
        }]
    }

    fn handle_protocol_message(
        &mut self,
        body: &[u8],
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        let Ok(msg) = serde_json::from_slice::<WorkerProtocolMessage>(body) else {
            return vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(400),
            }];
        };

        match msg {
            WorkerProtocolMessage::Poll(poll) => self.handle_poll(poll, responder),
            WorkerProtocolMessage::Result(result) => self.handle_result(result, responder),
            WorkerProtocolMessage::Progress(progress) => self.handle_progress(progress, responder),
            other => {
                let response = self.core.handle(other);
                vec![DaemonRequest::Respond {
                    responder,
                    response: protocol_response(response),
                }]
            }
        }
    }

    fn handle_poll(&mut self, poll: Poll, responder: HttpResponder) -> Vec<DaemonRequest> {
        let response = self
            .core
            .handle(WorkerProtocolMessage::Poll(poll.clone()))
            .expect("poll messages produce a response");

        if is_poll_timeout(&response) {
            let requested = poll.max_wait_ms.unwrap_or(self.max_poll_wait_ms);
            let wait_ms = requested.min(self.max_poll_wait_ms);
            let id = self.next_token();
            self.waiters.insert(id, PollWaiter { poll, responder });
            vec![DaemonRequest::StartPollTimer {
                id,
                delay: Duration::from_millis(wait_ms),
            }]
        } else {
            let mut requests = Vec::new();
            if let WorkerProtocolMessage::Assign(assign) = &response {
                requests.push(DaemonRequest::Log(assignment_log_line(
                    assign,
                    &poll.worker_id,
                )));
            }
            requests.push(DaemonRequest::Respond {
                responder,
                response: protocol_response(Some(response)),
            });
            requests
        }
    }

    fn handle_result(&mut self, result: JobResult, responder: HttpResponder) -> Vec<DaemonRequest> {
        let mut requests = Vec::new();
        // Capture full job context before the core completes and forgets the
        // job.
        let in_flight = self.core.in_flight_job(&result.job_id);
        let response = self
            .core
            .handle(WorkerProtocolMessage::Result(result.clone()));

        // Route only when the core accepted/completed the in-flight job.
        // Unknown, never-assigned, version-mismatched, and double-sent results
        // must not apply, retry, or drop beyond the core response.
        if let (Some(job), Some(WorkerProtocolMessage::Release(_))) = (in_flight, response.as_ref())
        {
            let disposition = result_disposition(&result);
            requests.push(DaemonRequest::Log(result_received_log_line(
                &result,
                result_disposition_log_value(disposition),
            )));

            match disposition {
                ResultDisposition::Apply => {
                    self.applying.insert(job.job_id.clone());
                    requests.push(DaemonRequest::RunApply { job, result });
                }
                // Apply retry bookkeeping (for example, releasing a claimed
                // source issue back to its ready queue) before the next webhook
                // wake or poll-backstop tick re-feeds the work through the
                // guarded scan path. The result is still logged as `rescan`: it
                // is not a terminal workflow outcome.
                ResultDisposition::DropForRescan => {
                    self.applying.insert(job.job_id.clone());
                    requests.push(DaemonRequest::RunApply { job, result });
                }
                ResultDisposition::Drop => {}
            }
        }

        requests.push(DaemonRequest::Respond {
            responder,
            response: protocol_response(response),
        });
        requests
    }

    fn handle_progress(
        &mut self,
        progress: JobProgress,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        // Fire-and-forget bookkeeping: ack with 204 either way, route to the
        // applier only when the correlation key names a job that is still in
        // flight (idempotent application makes late or duplicate delivery
        // harmless).
        let mut requests = Vec::new();
        match self
            .core
            .in_flight_job_by_correlation_key(&progress.correlation_key)
        {
            Some(job) => {
                requests.push(DaemonRequest::Log(progress_log_line(&job, &progress)));
                requests.push(DaemonRequest::RunProgressApply { job, progress });
            }
            None => {
                requests.push(DaemonRequest::Log(format!(
                    "engine: dropped progress for unknown correlation_key={} step={}",
                    progress.correlation_key, progress.step
                )));
            }
        }
        requests.push(DaemonRequest::Respond {
            responder,
            response: protocol_response(None),
        });
        requests
    }

    fn handle_webhook_delivery(
        &mut self,
        request: &HttpRequestData,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        let config = self.webhook.as_ref().expect("webhook config checked");
        let headers: BTreeMap<String, String> = request
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .collect();

        match parse_verified_webhook(&headers, &request.body, &config.secret) {
            Ok(hint) => {
                let token = self.next_token();
                self.webhook_waiters.insert(token, responder);
                vec![
                    DaemonRequest::Log(webhook_accepted_log_line(&hint)),
                    DaemonRequest::RunWakeScan { token, hint },
                ]
            }
            Err(WebhookError::InvalidSignature) => vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(401),
            }],
            Err(WebhookError::BadPayload(_)) => vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(400),
            }],
        }
    }

    pub(super) fn handle_enqueue(
        &mut self,
        job_id: String,
        role: String,
        repo: String,
        artifact: Artifact,
        job_payload: serde_json::Value,
    ) -> Vec<DaemonRequest> {
        let mut requests = Vec::new();
        let now = self.now;
        self.recently_applied.retain(|_, deadline| *deadline > now);
        if self.applying.contains(&job_id) {
            requests.push(DaemonRequest::Log(format!(
                "engine: skipped enqueue for job in apply window job_id={job_id}"
            )));
            return requests;
        }
        if self
            .recently_applied
            .get(&job_id)
            .is_some_and(|deadline| *deadline > now)
        {
            requests.push(DaemonRequest::Log(format!(
                "engine: skipped enqueue for recently applied job job_id={job_id}"
            )));
            return requests;
        }
        let role_for_saturation = role.clone();
        self.core
            .enqueue_job(job_id, role, repo, artifact, job_payload);
        if let Some(request) = self.role_saturation_request(&role_for_saturation) {
            requests.push(request);
        }
        requests.extend(self.fulfil_waiters());
        requests
    }

    /// Builds the §7 `role.saturated` request when the just-enqueued role is at
    /// its concurrency limit with same-role work now queued behind the holder.
    ///
    /// The pending wait list and the busy/idle decision come from the pure
    /// dispatch core ([`DaemonCore::role_saturation`]); the per-role concurrency
    /// figure is the number of in-flight slots the role currently holds (the
    /// limit it is hitting — `1` for the standalone single-slot roles). The
    /// `artifact.ref` strings are built here from each waiting job's repo and
    /// artifact coordinates. Returns `None` when the role is not saturated.
    fn role_saturation_request(&self, role: &str) -> Option<DaemonRequest> {
        let waiting_jobs = self.core.role_saturation(role);
        if waiting_jobs.is_empty() {
            return None;
        }
        let concurrency = self.core.in_flight_role_count(role).max(1);
        let waiting = waiting_jobs
            .iter()
            .filter_map(|(repo, artifact)| artifact_ref_string(repo, artifact))
            .collect::<Vec<_>>();
        if waiting.is_empty() {
            return None;
        }
        Some(DaemonRequest::RoleSaturated {
            role: role.to_string(),
            concurrency: u64::try_from(concurrency).unwrap_or(1),
            waiting,
        })
    }

    fn fulfil_waiters(&mut self) -> Vec<DaemonRequest> {
        let mut requests = Vec::new();
        let ids = self.waiters.keys().copied().collect::<Vec<_>>();

        for id in ids {
            let Some(waiter) = self.waiters.get(&id) else {
                continue;
            };

            let response = self
                .core
                .handle(WorkerProtocolMessage::Poll(waiter.poll.clone()))
                .expect("poll messages produce a response");

            if is_poll_timeout(&response) {
                continue;
            }

            let waiter = self
                .waiters
                .remove(&id)
                .expect("waiter exists after successful poll response");
            if let WorkerProtocolMessage::Assign(assign) = &response {
                requests.push(DaemonRequest::Log(assignment_log_line(
                    assign,
                    &waiter.poll.worker_id,
                )));
            }
            requests.push(DaemonRequest::Respond {
                responder: waiter.responder,
                response: protocol_response(Some(response)),
            });
        }
        requests
    }
}

/// Builds the canonical `artifact.ref` join key for a waiting job.
///
/// `repo` is the job's bare `owner/repo` path; the artifact's JSON `item` is a
/// numeric id and `kind` selects the issue (`owner/repo#n`) vs pull-request
/// (`owner/repo PR#n`) shape. Returns `None` for a non-numeric item or an
/// unrecognized kind, so the saturated-role wait list silently drops a malformed
/// entry rather than rendering a broken tag.
fn artifact_ref_string(repo: &str, artifact: &Artifact) -> Option<String> {
    let number = artifact.item.as_u64()?;
    let repo = strip_provider_scheme(repo);
    let item = match artifact.kind.as_str() {
        "issue" => WorkItemRef::issue(repo, number),
        "pull_request" => WorkItemRef::pull_request(repo, number),
        _ => return None,
    };
    Some(item.to_string())
}
