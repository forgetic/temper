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
use skein::runtime::RuntimeHandle;
use temper_protocol_agent::{StepProgress, StepState};
use temper_protocol_worker::{
    JobPlanPublication, JobPlanPublicationTarget, JobProgress, WORKER_PROTOCOL_VERSION,
    WorkerProtocolMessage,
};
use temper_worker_io::{HttpCall, build_http_client, http_call};

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
        plan_publication: progress
            .plan_publication
            .as_ref()
            .map(plan_publication_message),
    })
}

fn plan_publication_message(
    publication: &temper_protocol_agent::PlanPublication,
) -> JobPlanPublication {
    JobPlanPublication {
        summary: publication.summary.clone(),
        phases: publication.phases.clone(),
        target_repos: publication
            .target_repos
            .iter()
            .map(|target| JobPlanPublicationTarget {
                repo_path: target.repo_path.clone(),
                dir: target.dir.clone(),
                base_branch: target.base_branch.clone(),
                branch_hint: target.branch_hint.clone(),
            })
            .collect(),
    }
}

/// The production sink: log locally, relay to the daemon, never fail.
pub struct DaemonRelayProgressSink {
    handle: RuntimeHandle,
    http: Arc<HttpClient>,
    endpoint: String,
    worker_id: String,
}

impl DaemonRelayProgressSink {
    /// `daemon_url` is the base daemon URL; progress posts to
    /// `<daemon_url>/v1/message` like every other worker-protocol message.
    ///
    /// `handle` is the runtime's spawn capability, passed explicitly (the relay
    /// fires the post as a detached task; no ambient handle lookup).
    pub fn new(handle: RuntimeHandle, daemon_url: &str, worker_id: impl Into<String>) -> Self {
        Self {
            handle,
            http: build_http_client(),
            endpoint: format!("{}/v1/message", daemon_url.trim_end_matches('/')),
            worker_id: worker_id.into(),
        }
    }
}

impl ProgressSink for DaemonRelayProgressSink {
    fn report(&self, progress: StepProgress) {
        // Per-step progress trace, not a §7 catalog line (§5): keep it at debug.
        tracing::debug!(target: "temper_worker", "{}", crate::observability::step_progress_line(&progress));

        let message = progress_message(&self.worker_id, &progress);
        let call = HttpCall {
            method: "POST".to_string(),
            url: self.endpoint.clone(),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: serde_json::to_vec(&message).unwrap_or_default(),
        };
        // The relay holds the runtime's spawn capability explicitly; the post
        // is fire-and-forget on a detached task, honoring the never-fail
        // contract (transport trouble costs observability, not correctness).
        let http = Arc::clone(&self.http);
        let correlation = progress.correlation_key.clone();
        let step = progress.step;
        self.handle.spawn_with_cx(move |cx| async move {
            match http_call(&cx, &http, call).await {
                Ok(response) if (200..300).contains(&response.status) => {}
                Ok(response) => {
                    tracing::warn!(
                        target: "temper_worker",
                        correlation = %correlation,
                        step = %step,
                        status = response.status,
                        "progress relay dropped (daemon HTTP {})",
                        response.status
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        target: "temper_worker",
                        correlation = %correlation,
                        step = %step,
                        "progress relay dropped ({error})"
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_protocol_agent::{PlanPublication, PlanPublicationTarget};

    #[test]
    fn message_carries_exactly_the_step_progress_fields() {
        let progress = StepProgress {
            correlation_key: "pr-for-code-42".to_string(),
            step: 3,
            status: "implement the banner".to_string(),
            state: StepState::Done,
            pushed_sha: Some("abc123".to_string()),
            note: Some("done in one pass".to_string()),
            plan_publication: Some(PlanPublication {
                summary: "Ship the banner".to_string(),
                phases: vec!["implement the banner".to_string()],
                target_repos: vec![PlanPublicationTarget {
                    repo_path: "acme/demo".to_string(),
                    dir: "demo".to_string(),
                    base_branch: "main".to_string(),
                    branch_hint: Some("agent/banner".to_string()),
                }],
            }),
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
        let publication = message.plan_publication.expect("plan publication carried");
        assert_eq!(publication.summary, "Ship the banner");
        assert_eq!(publication.phases, vec!["implement the banner".to_string()]);
        assert_eq!(publication.target_repos[0].repo_path, "acme/demo");
        assert_eq!(
            publication.target_repos[0].branch_hint.as_deref(),
            Some("agent/banner")
        );
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
            plan_publication: None,
        };
        let WorkerProtocolMessage::Progress(message) = progress_message("w", &progress) else {
            panic!("expected a progress message");
        };
        assert_eq!(message.state, "started");
        assert_eq!(message.pushed_sha, None);
    }
}
