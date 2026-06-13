//! The worker's imperative shell.
//!
//! [`WorkerShell`] implements [`smith_io_engine::Executor`] for
//! [`WorkerMachine`](crate::worker_machine::WorkerMachine): it performs the I/O
//! each [`WorkerRequest`] asks for — POST worker-protocol messages to the daemon
//! over the skein HTTP client, run dispatched jobs through the
//! [`JobExecutor`], and arm timers — and feeds every result back into the
//! completion queue as a [`WorkerCompletion`]. It never calls into the machine.

use std::sync::Arc;

use skein::http::h1::http_client::HttpClient;
use skein::runtime::RuntimeHandle;
use smith_io_engine::{
    CqSender, HttpCall, HttpResponseData, arm_timer, build_http_client, http_call,
};
use temper_worker_protocol::WorkerProtocolMessage;

use crate::executor::{JobExecutor, job_result};
use crate::worker_machine::{WorkerCompletion, WorkerMachine, WorkerRequest};

/// Performs the worker's I/O on the skein runtime.
pub struct WorkerShell<E: JobExecutor> {
    handle: RuntimeHandle,
    cq: CqSender<WorkerCompletion>,
    http: Arc<HttpClient>,
    endpoint: String,
    worker_id: String,
    executor: Arc<E>,
}

impl<E: JobExecutor + Send + Sync + 'static> WorkerShell<E> {
    /// Builds the shell. `daemon_url` is the base daemon URL; the worker posts
    /// every message to `<daemon_url>/v1/message`.
    pub fn new(
        handle: RuntimeHandle,
        cq: CqSender<WorkerCompletion>,
        daemon_url: &str,
        worker_id: String,
        executor: Arc<E>,
    ) -> Self {
        let endpoint = format!("{}/v1/message", daemon_url.trim_end_matches('/'));
        Self {
            handle,
            cq,
            http: build_http_client(),
            endpoint,
            worker_id,
            executor,
        }
    }

    /// Spawn a daemon POST; map its decoded reply into `completion` and enqueue.
    fn post<F>(&self, message: &WorkerProtocolMessage, to_completion: F)
    where
        F: FnOnce(Result<Option<WorkerProtocolMessage>, String>) -> WorkerCompletion
            + Send
            + 'static,
    {
        let call = HttpCall {
            method: "POST".to_string(),
            url: self.endpoint.clone(),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: serde_json::to_vec(message).unwrap_or_default(),
        };
        let http = Arc::clone(&self.http);
        let cq = self.cq.clone();
        self.handle.spawn_with_cx(move |cx| async move {
            let decoded = match http_call(&cx, &http, call).await {
                Ok(response) => decode_reply(response),
                Err(error) => Err(error),
            };
            let _ = cq.send(to_completion(decoded));
        });
    }
}

impl<E: JobExecutor + Send + Sync + 'static> smith_io_engine::Executor<WorkerMachine>
    for WorkerShell<E>
{
    fn execute(&self, request: WorkerRequest) {
        match request {
            WorkerRequest::SendRegister(message) => {
                self.post(&message, |reply| {
                    WorkerCompletion::Registered(reply.map(|_| ()))
                });
            }
            WorkerRequest::SendPoll(message) => {
                self.post(&message, WorkerCompletion::PollReply);
            }
            WorkerRequest::SendResult { job_id, message } => {
                self.post(&message, move |reply| WorkerCompletion::ResultDelivered {
                    job_id,
                    outcome: reply.map(|_| ()),
                });
            }
            WorkerRequest::SendHeartbeat(message) => {
                self.post(&message, |reply| {
                    WorkerCompletion::HeartbeatDelivered(reply.map(|_| ()))
                });
            }
            WorkerRequest::RunJob(assign) => {
                let executor = Arc::clone(&self.executor);
                let cq = self.cq.clone();
                let worker_id = self.worker_id.clone();
                let job_id = assign.job_id.clone();
                self.handle.spawn(async move {
                    let outcome = executor.execute(assign).await;
                    let result = job_result(&worker_id, &job_id, outcome);
                    let _ = cq.send(WorkerCompletion::JobFinished { job_id, result });
                });
            }
            WorkerRequest::ArmPollTimer(delay) => {
                arm_timer(&self.handle, &self.cq, delay, || {
                    WorkerCompletion::PollTimer
                });
            }
            WorkerRequest::ArmHeartbeatTimer(delay) => {
                arm_timer(&self.handle, &self.cq, delay, || {
                    WorkerCompletion::HeartbeatTimer
                });
            }
            WorkerRequest::Log(line) => {
                eprintln!("{line}");
            }
        }
    }
}

/// Decode a daemon HTTP response into the worker-protocol reply the machine
/// expects: `Ok(None)` for 204/empty, `Ok(Some(message))` for a 200 JSON body,
/// `Err` for a non-success status or malformed JSON. Mirrors the semantics of
/// the previous reqwest `WorkerClient::send`.
fn decode_reply(response: HttpResponseData) -> Result<Option<WorkerProtocolMessage>, String> {
    match response.status {
        204 => Ok(None),
        200 => {
            if response.body.is_empty() {
                return Ok(None);
            }
            serde_json::from_slice::<WorkerProtocolMessage>(&response.body)
                .map(Some)
                .map_err(|error| {
                    let body = String::from_utf8_lossy(&response.body);
                    format!(
                        "daemon response was not valid worker protocol JSON: {error}; body: {body}"
                    )
                })
        }
        status => {
            let body = String::from_utf8_lossy(&response.body);
            Err(format!("daemon returned HTTP {status}: {body}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &[u8]) -> HttpResponseData {
        HttpResponseData {
            status,
            headers: Vec::new(),
            body: body.to_vec(),
        }
    }

    #[test]
    fn decode_204_is_none() {
        assert_eq!(decode_reply(response(204, b"")), Ok(None));
    }

    #[test]
    fn decode_200_empty_is_none() {
        assert_eq!(decode_reply(response(200, b"")), Ok(None));
    }

    #[test]
    fn decode_200_message_parses() {
        let release = serde_json::json!({
            "type": "release",
            "protocol_version": temper_worker_protocol::WORKER_PROTOCOL_VERSION,
            "worker_id": "w1",
            "job_id": "j1",
            "disposition": "accepted",
            "message": null,
        });
        let bytes = serde_json::to_vec(&release).unwrap();
        let decoded = decode_reply(response(200, &bytes)).expect("decodes");
        assert!(matches!(decoded, Some(WorkerProtocolMessage::Release(_))));
    }

    #[test]
    fn decode_non_success_is_err() {
        let error = decode_reply(response(500, b"boom")).expect_err("error status");
        assert!(error.contains("HTTP 500"));
        assert!(error.contains("boom"));
    }

    #[test]
    fn decode_malformed_200_is_err() {
        let error = decode_reply(response(200, b"not json")).expect_err("bad json");
        assert!(error.contains("not valid worker protocol JSON"));
    }
}
