//! The worker → daemon step-progress relay (the second half of the
//! agent → worker → daemon → forge path).
//!
//! [`DaemonRelayProgressSink`] is the production [`ProgressSink`]: it logs
//! each checkpoint locally (same line the logging sink printed) and
//! fire-and-forgets a `progress` worker-protocol message to the daemon, which
//! applies it to the forge idempotently keyed by
//! `(correlation_key, step, state)`. Per the sink contract, transport trouble
//! is swallowed: a slow or unreachable daemon must never stall or fail the
//! agent turn, whose real product is the result + the pushed commits — a
//! dropped checkpoint costs observability, not correctness.

use std::sync::Arc;

use skein::http::h1::http_client::HttpClient;
use smith_agent_protocol::{StepProgress, StepState};
use smith_io_engine::{HttpCall, build_http_client, http_call};
use temper_worker_protocol::{JobProgress, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage};

use crate::agent_runner::ProgressSink;

/// Builds the wire message for one checkpoint. Pure; carries exactly the
/// `StepProgress` fields plus the protocol envelope (the daemon resolves the
/// in-flight job from `correlation_key` — the one cross-plane identifier).
pub fn progress_message(worker_id: &str, progress: &StepProgress) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Progress(JobProgress {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        correlation_key: progress.correlation_key.clone(),
        step: progress.step,
        status: progress.status.clone(),
        state: match progress.state {
            StepState::Started => "started",
            StepState::Done => "done",
        }
        .to_string(),
        pushed_sha: progress.pushed_sha.clone(),
        note: progress.note.clone(),
    })
}

/// The production sink: log locally, relay to the daemon, never fail.
pub struct DaemonRelayProgressSink {
    http: Arc<HttpClient>,
    endpoint: String,
    worker_id: String,
}

impl DaemonRelayProgressSink {
    /// `daemon_url` is the base daemon URL; progress posts to
    /// `<daemon_url>/v1/message` like every other worker-protocol message.
    pub fn new(daemon_url: &str, worker_id: impl Into<String>) -> Self {
        Self {
            http: build_http_client(),
            endpoint: format!("{}/v1/message", daemon_url.trim_end_matches('/')),
            worker_id: worker_id.into(),
        }
    }
}

impl ProgressSink for DaemonRelayProgressSink {
    fn report(&self, progress: StepProgress) {
        println!("{}", crate::observability::step_progress_line(&progress));

        let message = progress_message(&self.worker_id, &progress);
        let call = HttpCall {
            method: "POST".to_string(),
            url: self.endpoint.clone(),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: serde_json::to_vec(&message).unwrap_or_default(),
        };
        // report() is called from inside an engine task (the runner relays
        // markers there), so the runtime handle is ambient; without one we
        // keep the local log line and drop the relay, honoring the
        // never-fail contract.
        let Some(handle) = skein::runtime::Runtime::current_handle() else {
            eprintln!(
                "smith-worker: progress relay skipped (no runtime handle) correlation={} step={}",
                progress.correlation_key, progress.step
            );
            return;
        };
        let http = Arc::clone(&self.http);
        let correlation = progress.correlation_key.clone();
        let step = progress.step;
        handle.spawn_with_cx(move |cx| async move {
            match http_call(&cx, &http, call).await {
                Ok(response) if (200..300).contains(&response.status) => {}
                Ok(response) => {
                    eprintln!(
                        "smith-worker: progress relay dropped (daemon HTTP {}) correlation={correlation} step={step}",
                        response.status
                    );
                }
                Err(error) => {
                    eprintln!(
                        "smith-worker: progress relay dropped ({error}) correlation={correlation} step={step}"
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_carries_exactly_the_step_progress_fields() {
        let progress = StepProgress {
            correlation_key: "pr-for-code-42".to_string(),
            step: 3,
            status: "implement the banner".to_string(),
            state: StepState::Done,
            pushed_sha: Some("abc123".to_string()),
            note: Some("done in one pass".to_string()),
        };
        let WorkerProtocolMessage::Progress(message) = progress_message("w1", &progress) else {
            panic!("expected a progress message");
        };
        assert_eq!(message.protocol_version, WORKER_PROTOCOL_VERSION);
        assert_eq!(message.worker_id, "w1");
        assert_eq!(message.correlation_key, "pr-for-code-42");
        assert_eq!(message.step, 3);
        assert_eq!(message.status, "implement the banner");
        assert_eq!(message.state, "done");
        assert_eq!(message.pushed_sha.as_deref(), Some("abc123"));
        assert_eq!(message.note.as_deref(), Some("done in one pass"));
    }

    #[test]
    fn started_state_maps_to_wire_string() {
        let progress = StepProgress {
            correlation_key: "k".to_string(),
            step: 1,
            status: "start".to_string(),
            state: StepState::Started,
            pushed_sha: None,
            note: None,
        };
        let WorkerProtocolMessage::Progress(message) = progress_message("w", &progress) else {
            panic!("expected a progress message");
        };
        assert_eq!(message.state, "started");
        assert_eq!(message.pushed_sha, None);
    }
}
