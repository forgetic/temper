// SPDX-License-Identifier: MPL-2.0

//! Per-message handler logic for [`DaemonMachine`]: HTTP routing, worker
//! protocol dispatch, webhook delivery verification, enqueue gating, and
//! long-poll waiter fulfilment. Pure transitions returning [`DaemonRequest`]s.

use std::collections::BTreeSet;
use std::time::Duration;

use temper_engine_io::http::{HttpRequestData, HttpResponder, HttpResponseData};
use temper_log::{WorkItemRef, strip_provider_scheme};
use temper_protocol_worker::{
    Artifact, Assign, ErrorCode, ForgeContextErrorCode, Heartbeat, JobResult, Poll, ProtocolError,
    PullRequestFreshness, Register, WORKER_AUTHORIZATION_HEADER, WORKER_PROTOCOL_VERSION,
    WorkerAuth, WorkerProtocolMessage,
};

use super::context_transport::malformed_context_response;
use super::machine::{DaemonMachine, DaemonRequest, DeferredEnqueue, PollWaiter};
use super::protocol::{
    is_poll_timeout, protocol_response, register_log_line, result_disposition,
    result_disposition_log_value, result_received_log_line,
};
use super::state_dto::{DaemonStateSnapshot, JobDto};
use crate::InFlightJob;

impl DaemonMachine {
    pub(super) fn handle_http(
        &mut self,
        request: HttpRequestData,
        responder: HttpResponder,
        trusted_transport: bool,
    ) -> Vec<DaemonRequest> {
        if crate::trace_query::is_trace_uri(&request.uri) {
            return vec![DaemonRequest::RunTraceQuery { request, responder }];
        }
        match (request.method.as_str(), request.uri.as_str()) {
            ("POST", "/v1/message") => {
                self.handle_protocol_message(&request, responder, trusted_transport)
            }
            ("POST", "/v1/pr-freshness") => {
                self.handle_pull_request_freshness_check(&request.body, responder)
            }
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
            Some(job) => HttpResponseData::json(
                200,
                &JobDto::from_in_flight(&job, self.core.worker_job_report(job_id)).to_json(),
            ),
            None => HttpResponseData::status_only(404),
        };
        vec![DaemonRequest::Respond {
            responder,
            response,
        }]
    }

    fn handle_pull_request_freshness_check(
        &mut self,
        body: &[u8],
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        let Ok(check) = serde_json::from_slice::<PullRequestFreshness>(body) else {
            return vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(400),
            }];
        };
        vec![DaemonRequest::RunPullRequestFreshnessCheck { check, responder }]
    }

    fn handle_protocol_message(
        &mut self,
        request: &HttpRequestData,
        responder: HttpResponder,
        trusted_transport: bool,
    ) -> Vec<DaemonRequest> {
        let msg = match serde_json::from_slice::<WorkerProtocolMessage>(&request.body) {
            Ok(message) => message,
            Err(_) => {
                if let Some((response, audit)) = malformed_context_response(&request.body) {
                    return vec![DaemonRequest::RespondContext {
                        response,
                        audit,
                        responder,
                    }];
                }
                return vec![DaemonRequest::Respond {
                    responder,
                    response: HttpResponseData::status_only(400),
                }];
            }
        };
        let auth = match worker_auth_from_headers(&request.headers) {
            Ok(auth) => auth,
            Err(()) => {
                return match msg {
                    WorkerProtocolMessage::FetchContext(fetch) => self.context_error_requests(
                        fetch,
                        "unknown",
                        ForgeContextErrorCode::NotAuthorized,
                        responder,
                    ),
                    _ => vec![DaemonRequest::Respond {
                        responder,
                        response: HttpResponseData::status_only(401),
                    }],
                };
            }
        };

        match msg {
            WorkerProtocolMessage::Register(register) => {
                self.handle_register(register, auth, responder)
            }
            WorkerProtocolMessage::Poll(poll) => self.handle_poll(poll, auth, responder),
            WorkerProtocolMessage::Heartbeat(heartbeat) => {
                self.handle_heartbeat(heartbeat, auth, responder)
            }
            WorkerProtocolMessage::Result(result) => self.handle_result(result, auth, responder),
            WorkerProtocolMessage::FetchContext(fetch) => {
                self.handle_fetch_context(fetch, auth, responder)
            }
            WorkerProtocolMessage::ActivityBatch(batch) => {
                self.handle_activity_batch(batch, auth, trusted_transport, responder)
            }
            other => match self.core.handle_authenticated(other, auth.as_ref()) {
                Ok(response) => vec![DaemonRequest::Respond {
                    responder,
                    response: protocol_response(response),
                }],
                Err(_) => vec![DaemonRequest::Respond {
                    responder,
                    response: HttpResponseData::status_only(401),
                }],
            },
        }
    }

    fn handle_register(
        &mut self,
        register: Register,
        auth: Option<WorkerAuth>,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        let response = match self.core.handle_authenticated(
            WorkerProtocolMessage::Register(register.clone()),
            auth.as_ref(),
        ) {
            Ok(response) => response,
            Err(_) => {
                return vec![DaemonRequest::Respond {
                    responder,
                    response: HttpResponseData::status_only(401),
                }];
            }
        };
        let mut requests = Vec::new();
        if response.is_none() {
            requests.push(DaemonRequest::Log(register_log_line(&register)));
        }
        requests.push(DaemonRequest::Respond {
            responder,
            response: protocol_response(response),
        });
        requests
    }

    fn handle_heartbeat(
        &mut self,
        heartbeat: Heartbeat,
        auth: Option<WorkerAuth>,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        let (response, recovery) = match self
            .core
            .handle_authenticated_heartbeat(heartbeat, auth.as_ref())
        {
            Ok(outcome) => outcome,
            Err(_) => {
                return vec![DaemonRequest::Respond {
                    responder,
                    response: HttpResponseData::status_only(401),
                }];
            }
        };

        let mut assignments = Vec::new();
        for job_id in recovery.matched_job_ids {
            let Some(job) = self.core.in_flight_job(&job_id) else {
                continue;
            };
            let Some(context) = self.assignment_contexts.get(&job_id).cloned() else {
                continue;
            };
            assignments.push((job, context));
        }
        vec![DaemonRequest::RunHeartbeatsAndRespond {
            assignments,
            responder,
            response: protocol_response(response),
        }]
    }

    fn handle_poll(
        &mut self,
        poll: Poll,
        auth: Option<WorkerAuth>,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        if self.shutting_down {
            return vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(204),
            }];
        }
        if self.startup_recovery {
            let id = self.next_token();
            self.waiters.insert(
                id,
                PollWaiter {
                    poll,
                    auth,
                    responder,
                },
            );
            return Vec::new();
        }
        let response = match self
            .core
            .reserve_authenticated_poll(poll.clone(), auth.as_ref())
        {
            Ok(response) => response,
            Err(_) => {
                return vec![DaemonRequest::Respond {
                    responder,
                    response: HttpResponseData::status_only(401),
                }];
            }
        };

        if is_poll_timeout(&response) {
            let requested = poll.max_wait_ms.unwrap_or(self.max_poll_wait_ms);
            let wait_ms = requested.min(self.max_poll_wait_ms);
            let id = self.next_token();
            self.waiters.insert(
                id,
                PollWaiter {
                    poll,
                    auth,
                    responder,
                },
            );
            vec![DaemonRequest::StartPollTimer {
                id,
                delay: Duration::from_millis(wait_ms),
            }]
        } else {
            self.poll_response_requests(response, &poll.worker_id, responder)
        }
    }

    pub(super) fn poll_response_requests(
        &self,
        response: WorkerProtocolMessage,
        worker_id: &str,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        match response {
            WorkerProtocolMessage::Assign(assign) => {
                let job = in_flight_job_from_assign(&assign);
                vec![DaemonRequest::RunClaim {
                    job,
                    worker_id: worker_id.to_string(),
                    daemon_boot_id: self.daemon_boot_id.clone(),
                    assign,
                    responder,
                }]
            }
            response => vec![DaemonRequest::Respond {
                responder,
                response: protocol_response(Some(response)),
            }],
        }
    }

    fn handle_result(
        &mut self,
        result: JobResult,
        auth: Option<WorkerAuth>,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        match self.core.authenticate_result(&result, auth.as_ref()) {
            Err(_) => {
                return vec![DaemonRequest::Respond {
                    responder,
                    response: HttpResponseData::status_only(401),
                }];
            }
            Ok(Some(response)) => {
                return vec![DaemonRequest::Respond {
                    responder,
                    response: protocol_response(Some(response)),
                }];
            }
            Ok(None) => {}
        }

        let key = (result.job_id.clone(), result.attempt_id.clone());

        // A lost acknowledgement may replay after the exact apply completed.
        // Retain the applied payload so only a byte-for-byte equivalent result
        // receives the prior release and no Forge work runs twice.
        if let Some((applied, response)) = self.completed_results.get(&key) {
            let response = if applied == &result {
                protocol_response(Some(response.clone()))
            } else {
                protocol_response(Some(WorkerProtocolMessage::Error(ProtocolError {
                    protocol_version: WORKER_PROTOCOL_VERSION,
                    code: ErrorCode::MalformedMessage,
                    message: "different result reused a completed assignment attempt".to_string(),
                    retry_after_ms: None,
                    job_id: Some(result.job_id.clone()),
                })))
            };
            return vec![DaemonRequest::Respond {
                responder,
                response,
            }];
        }
        if let Some(pending) = self.pending_results.get(&key) {
            let response = if pending == &result {
                HttpResponseData::status_only(503)
            } else {
                HttpResponseData::status_only(422)
            };
            return vec![DaemonRequest::Respond {
                responder,
                response,
            }];
        }

        let job = match self.core.result_job(&result) {
            Err(response) => {
                return vec![DaemonRequest::Respond {
                    responder,
                    response: protocol_response(Some(response)),
                }];
            }
            Ok(job) => job,
        };

        // A recovered claim keeps its prior daemon boot identity until exact
        // release. Retry every delivery through `apply_recovered`, not merely
        // the first one: a transient failure may happen before the lease
        // decorator has reattached the claim to its process-local state.
        let recovered_context = self
            .assignment_contexts
            .get(&job.job_id)
            .filter(|context| context.daemon_boot_id != self.daemon_boot_id)
            .cloned();
        let disposition = result_disposition(&result);
        let mut requests = vec![DaemonRequest::Log(result_received_log_line(
            &result,
            result_disposition_log_value(disposition),
        ))];
        if self.applying.is_empty() {
            let decisions = self.wake_coordinator.begin_apply();
            requests.extend(self.wake_decision_requests(decisions));
        }
        self.applying.insert(job.job_id.clone());
        self.pending_results.insert(key, result.clone());
        requests.push(DaemonRequest::RunApplyAndRespond {
            job,
            result,
            recovered_context,
            responder,
        });
        requests
    }

    pub(super) fn handle_enqueue(
        &mut self,
        job_id: String,
        role: String,
        repo: String,
        artifact: Artifact,
        job_payload: serde_json::Value,
    ) -> Vec<DaemonRequest> {
        if self.shutting_down {
            return Vec::new();
        }
        if self.startup_recovery {
            self.deferred_enqueues.push(DeferredEnqueue {
                job_id,
                role,
                repo,
                artifact,
                job_payload,
            });
            return Vec::new();
        }
        let mut requests = Vec::new();
        let now = self.now;
        self.recently_applied.retain(|_, deadline| *deadline > now);
        self.retry_backoff_until
            .retain(|_, deadline| *deadline > now);
        // Result application can create and then finish wiring new artifacts
        // across several Forge calls. Suppress every enqueue until all active
        // applies finish so a webhook or poll scan cannot dispatch a partially
        // created child before its dependency links and blocked label exist.
        if !self.applying.is_empty() {
            requests.push(DaemonRequest::Log(format!(
                "engine: skipped enqueue during result apply window job_id={job_id} active_applies={}",
                self.applying.len()
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
        if self
            .retry_backoff_until
            .get(&job_id)
            .is_some_and(|deadline| *deadline > now)
        {
            let attempt = self.retry_attempts.get(&job_id).copied().unwrap_or(1);
            requests.push(DaemonRequest::Log(format!(
                "engine: skipped enqueue during retry backoff job_id={job_id} attempt={attempt}"
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

    pub(super) fn handle_reconcile_pending_role_jobs(
        &mut self,
        repo: String,
        role: String,
        current_job_ids: BTreeSet<String>,
    ) -> Vec<DaemonRequest> {
        let pruned = self
            .core
            .retain_pending_jobs_for_scope(&repo, &role, &current_job_ids);
        if pruned.is_empty() {
            return Vec::new();
        }

        vec![DaemonRequest::Log(format!(
            "engine: pruned stale pending jobs repo={repo} role={role} count={} job_ids={}",
            pruned.len(),
            pruned.join(",")
        ))]
    }

    pub(super) fn handle_reconcile_pending_targeted_role_jobs(
        &mut self,
        repo: String,
        role: String,
        artifact: Artifact,
        current_job_ids: BTreeSet<String>,
    ) -> Vec<DaemonRequest> {
        let pruned =
            self.core
                .retain_pending_jobs_for_artifact(&repo, &role, &artifact, &current_job_ids);
        if pruned.is_empty() {
            return Vec::new();
        }

        vec![DaemonRequest::Log(format!(
            "engine: pruned stale targeted pending jobs repo={repo} role={role} artifact_kind={} artifact_item={} count={} job_ids={}",
            artifact.kind,
            artifact.item,
            pruned.len(),
            pruned.join(",")
        ))]
    }

    /// Builds the §7 `role.saturated` request when the just-enqueued role is at
    /// its configured finite concurrency limit with same-role work queued.
    /// The concurrency figure and ordered pending entries both come from the
    /// dispatch core's structured saturation result.
    fn role_saturation_request(&self, role: &str) -> Option<DaemonRequest> {
        let saturation = self.core.role_saturation(role)?;
        let waiting = saturation
            .pending
            .iter()
            .filter_map(|(repo, artifact)| artifact_ref_string(repo, artifact))
            .collect::<Vec<_>>();
        if waiting.is_empty() {
            return None;
        }
        Some(DaemonRequest::RoleSaturated {
            role: role.to_string(),
            concurrency: u64::from(saturation.concurrency),
            waiting,
        })
    }

    pub(super) fn fulfil_waiters(&mut self) -> Vec<DaemonRequest> {
        if self.startup_recovery || self.shutting_down {
            return Vec::new();
        }
        let mut requests = Vec::new();
        let ids = self.waiters.keys().copied().collect::<Vec<_>>();

        for id in ids {
            let Some(waiter) = self.waiters.get(&id) else {
                continue;
            };

            let response = match self
                .core
                .reserve_authenticated_poll(waiter.poll.clone(), waiter.auth.as_ref())
            {
                Ok(response) => response,
                Err(_) => {
                    let waiter = self
                        .waiters
                        .remove(&id)
                        .expect("waiter exists after auth failure");
                    requests.push(DaemonRequest::Respond {
                        responder: waiter.responder,
                        response: HttpResponseData::status_only(401),
                    });
                    continue;
                }
            };

            if is_poll_timeout(&response) {
                continue;
            }

            let waiter = self
                .waiters
                .remove(&id)
                .expect("waiter exists after successful poll response");
            requests.extend(self.poll_response_requests(
                response,
                &waiter.poll.worker_id,
                waiter.responder,
            ));
        }
        requests
    }
}

fn worker_auth_from_headers(headers: &[(String, String)]) -> Result<Option<WorkerAuth>, ()> {
    let Some((_, value)) = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(WORKER_AUTHORIZATION_HEADER))
    else {
        return Ok(None);
    };
    WorkerAuth::from_authorization_header(value)
        .map(Some)
        .ok_or(())
}

fn in_flight_job_from_assign(assign: &Assign) -> InFlightJob {
    InFlightJob {
        job_id: assign.job_id.clone(),
        attempt_id: assign.attempt_id.clone(),
        role: assign.role.clone(),
        repo: assign.repo.clone(),
        artifact: assign.artifact.clone(),
        job_payload: assign.job_payload.clone(),
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

#[cfg(test)]
#[path = "handlers_tests.rs"]
mod tests;
