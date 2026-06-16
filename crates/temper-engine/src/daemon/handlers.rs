// SPDX-License-Identifier: MPL-2.0

//! Per-message handler logic for [`DaemonMachine`]: HTTP routing, worker
//! protocol dispatch, webhook delivery verification, enqueue gating, and
//! long-poll waiter fulfilment. Pure transitions returning [`DaemonRequest`]s.

use std::collections::BTreeMap;
use std::time::Duration;

use temper_engine_io::http::{HttpRequestData, HttpResponder, HttpResponseData};
use temper_worker_protocol::{Artifact, JobProgress, JobResult, Poll, WorkerProtocolMessage};

use crate::webhook::{WebhookError, parse_verified_webhook, webhook_accepted_log_line};

use super::machine::{DaemonMachine, DaemonRequest, PollWaiter};
use super::protocol::{
    ResultDisposition, assignment_log_line, is_poll_timeout, progress_log_line, protocol_response,
    result_disposition, result_disposition_log_value, result_received_log_line,
};

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
            _ => vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(404),
            }],
        }
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
                // Let the next webhook wake or poll-backstop tick re-feed this
                // through the guarded scan path instead of hot re-enqueuing.
                ResultDisposition::DropForRescan => {}
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
        self.core
            .enqueue_job(job_id, role, repo, artifact, job_payload);
        requests.extend(self.fulfil_waiters());
        requests
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
