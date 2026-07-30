use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::task::{Poll, Waker};

use serde_json::Value;
use temper_protocol_activity::ModelFailureV1;
use temper_protocol_worker::{
    Assign, Branch, Failure, FailureClass, JobChild, JobResult, RepoOutcome, ResultStatus,
    SessionRecoveryEvidenceV1, WORKER_PROTOCOL_VERSION,
};

use crate::agent_runner::JobProgressReporter;

#[derive(Clone, Debug, PartialEq)]
pub enum JobOutcome {
    Success {
        /// Per-repo head products — one per writable repo that produced a diff.
        /// The daemon opens one pull request per entry. A coding job that wrote
        /// to a single repo produces exactly one outcome.
        repos: Vec<RepoOutcome>,
        /// Optional agent-authored implementation PR title for no-verdict
        /// success results.
        title: Option<String>,
        /// Optional agent-authored implementation PR report body for no-verdict
        /// success results.
        body: Option<String>,
        summary: Option<String>,
        /// Optional structured metadata for daemon-side application.
        details: Option<Value>,
    },
    Verdict {
        verdict: String,
        /// Optional agent-authored title for verdict transitions that create a
        /// pull request from metadata instead of a pushed workspace head.
        title: Option<String>,
        body: Option<String>,
        summary: Option<String>,
        children: Vec<JobChild>,
        /// Worker-owned structured metadata for daemon-side verdict gates.
        details: Option<Value>,
    },
    Failure {
        class: FailureClass,
        message: String,
        /// Canonical model terminal retained even when activity capture is absent.
        model_failure: Option<ModelFailureV1>,
        /// Optional durable session-policy decision associated with this failure.
        session_recovery: Option<SessionRecoveryEvidenceV1>,
    },
}

/// Stable identity for one daemon assignment plus the worker-local incarnation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobAttempt {
    pub id: String,
    pub generation: u64,
}

/// Shared publication fence. Closing it is irreversible and immediately makes
/// every late result, validation, git, and side-channel path non-authoritative.
#[derive(Clone, Debug)]
pub struct AttemptFence {
    open: Arc<AtomicBool>,
}

impl AttemptFence {
    pub fn open() -> Self {
        Self {
            open: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn close(&self) {
        self.open.store(false, Ordering::Release);
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }
}

mod cancellation;
pub use cancellation::{
    CancellationOutcome, JobCancellation, JobCancellationOwner, JobCancellationRequest, JobCleanup,
    ResourceJoinReport, ResourceJoinStatus, TerminalTraceBlocker, TerminalTraceBlockerState,
};
pub(crate) use cancellation::{JobCleanupObserver, JobContainmentObservation};

/// Worker-owned controls supplied to every layer of one job execution.
#[derive(Clone, Debug)]
pub struct JobExecutionContext {
    pub attempt: JobAttempt,
    pub fence: AttemptFence,
    pub cancellation: JobCancellation,
    pub progress: JobProgressReporter,
}

impl JobExecutionContext {
    pub fn unsupervised(assign: &Assign) -> Self {
        let attempt_id = assign
            .attempt_id
            .clone()
            .unwrap_or_else(|| assign.job_id.clone());
        Self {
            attempt: JobAttempt {
                id: attempt_id.clone(),
                generation: 0,
            },
            fence: AttemptFence::open(),
            cancellation: JobCancellation::default(),
            progress: JobProgressReporter::noop(attempt_id),
        }
    }
}

pub trait JobExecutor {
    fn execute(
        &self,
        assign: Assign,
        context: JobExecutionContext,
    ) -> impl std::future::Future<Output = JobOutcome> + Send;
}

#[derive(Clone, Debug, PartialEq)]
pub struct StubExecutor {
    mode: StubMode,
}

#[derive(Clone, Debug, PartialEq)]
enum StubMode {
    Success,
    Failure {
        class: FailureClass,
        message: String,
    },
}

impl StubExecutor {
    pub fn success() -> Self {
        Self {
            mode: StubMode::Success,
        }
    }

    pub fn failure(class: FailureClass, message: impl Into<String>) -> Self {
        Self {
            mode: StubMode::Failure {
                class,
                message: message.into(),
            },
        }
    }
}

impl StubExecutor {
    /// Compatibility entry point for direct executor tests and embedders that
    /// do not run the worker watchdog.
    pub fn execute(&self, assign: Assign) -> impl Future<Output = JobOutcome> + Send {
        let context = JobExecutionContext::unsupervised(&assign);
        <Self as JobExecutor>::execute(self, assign, context)
    }
}

impl JobExecutor for StubExecutor {
    fn execute(
        &self,
        assign: Assign,
        _context: JobExecutionContext,
    ) -> impl std::future::Future<Output = JobOutcome> + Send {
        let mode = self.mode.clone();
        async move {
            match mode {
                StubMode::Success => JobOutcome::Success {
                    repos: vec![RepoOutcome {
                        repo: assign.repo.clone(),
                        branch: Branch {
                            name: format!("temper-worker/stub/{}", assign.job_id),
                            head_sha: "0000000000000000000000000000000000000000".to_string(),
                        },
                    }],
                    title: None,
                    body: None,
                    summary: Some("stub executor completed without doing IO".to_string()),
                    details: None,
                },
                StubMode::Failure { class, message } => JobOutcome::Failure {
                    class,
                    message,
                    model_failure: None,
                    session_recovery: None,
                },
            }
        }
    }
}

pub fn job_result(worker_id: &str, job_id: &str, outcome: JobOutcome) -> JobResult {
    job_result_for_attempt(worker_id, job_id, None, outcome)
}

/// Builds a terminal result fenced to the exact daemon assignment attempt.
pub fn job_result_for_attempt(
    worker_id: &str,
    job_id: &str,
    attempt_id: Option<String>,
    outcome: JobOutcome,
) -> JobResult {
    let base = JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        attempt_id,
        status: ResultStatus::Success,
        repos: Vec::new(),
        verdict: None,
        title: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: None,
        details: None,
    };
    match outcome {
        JobOutcome::Success {
            repos,
            title,
            body,
            summary,
            details,
        } => JobResult {
            status: ResultStatus::Success,
            repos,
            title,
            body,
            summary,
            details,
            ..base
        },
        JobOutcome::Verdict {
            verdict,
            title,
            body,
            summary,
            children,
            details,
        } => JobResult {
            status: ResultStatus::Success,
            verdict: Some(verdict),
            title,
            body,
            children,
            summary,
            details,
            ..base
        },
        JobOutcome::Failure {
            class,
            message,
            model_failure,
            session_recovery,
        } => {
            let mut failure = Failure {
                class,
                message,
                model_failure,
                session_recovery,
            };
            failure.normalize_evidence(base.attempt_id.as_deref());
            if let Some(recovery) = &failure.session_recovery {
                failure.class = match recovery.action {
                    temper_protocol_worker::SessionRecoveryActionV1::RetryCurrentSession
                    | temper_protocol_worker::SessionRecoveryActionV1::RotateSession
                    | temper_protocol_worker::SessionRecoveryActionV1::ProviderDeferred => {
                        FailureClass::Transient
                    }
                    temper_protocol_worker::SessionRecoveryActionV1::ParkForHuman => {
                        FailureClass::Permanent
                    }
                };
            }
            JobResult {
                status: ResultStatus::Failure,
                failure: Some(failure),
                ..base
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;
    use temper_protocol_worker::Artifact;

    use super::*;

    fn assign(job_id: &str) -> Assign {
        Assign {
            protocol_version: WORKER_PROTOCOL_VERSION,
            trace_context: None,
            job_id: job_id.to_string(),
            attempt_id: Some(format!("attempt-{job_id}")),
            role: "coder".to_string(),
            repo: "ai/temper".to_string(),
            artifact: Artifact {
                item: json!(78),
                kind: "issue".to_string(),
            },
            job_payload: json!({}),
        }
    }

    #[test]
    fn compatibility_trace_pending_reason_is_typed_bounded_and_redacted() {
        let cancellation = JobCancellation::default();
        let observed = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&observed);
        cancellation.set_cleanup_observer(move |observation| {
            let JobContainmentObservation::TerminalTracePending(blocker) = observation else {
                panic!("expected terminal trace blocker");
            };
            *captured.lock().expect("captured blocker") = Some(blocker);
        });

        cancellation.quiescence_pending(format!(
            "credential=secret-token-sentinel{}",
            "x".repeat(temper_protocol_worker::MAX_SHUTDOWN_IDENTIFIER_BYTES + 1)
        ));

        let blocker = observed
            .lock()
            .expect("captured blocker")
            .clone()
            .expect("compatibility blocker");
        assert_eq!(blocker.state(), TerminalTraceBlockerState::Compatibility);
        assert_eq!(blocker.run_id(), None);
        assert_eq!(blocker.sequence(), None);
        assert_eq!(blocker.to_string(), "[redacted]");
    }

    #[test]
    fn cancellation_handshake_preserves_every_escalation_and_one_cleanup() {
        let cancellation = JobCancellation::default();
        cancellation.hard_kill();
        let waker = Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        assert_eq!(
            cancellation.poll_request(None, &mut cx),
            Poll::Ready(JobCancellationRequest::Graceful)
        );
        assert_eq!(
            cancellation.poll_request(Some(JobCancellationRequest::Graceful), &mut cx),
            Poll::Ready(JobCancellationRequest::ForcedTermination)
        );
        assert_eq!(
            cancellation.poll_request(Some(JobCancellationRequest::ForcedTermination), &mut cx),
            Poll::Ready(JobCancellationRequest::HardKill)
        );
        let cleanup = JobCleanup::no_process(Some(CancellationOutcome::HardKill));
        assert!(cancellation.record_cleanup(cleanup.clone()));
        assert!(
            !cancellation
                .record_cleanup(JobCleanup::no_process(Some(CancellationOutcome::Graceful,)))
        );
        assert_eq!(cancellation.cleanup(), Some(cleanup));

        let owned = JobCancellation::default();
        let owner = owned.register_async_owner();
        let mut run = Box::pin(owned.run_to_quiescence(std::future::pending::<()>()));
        owned.cancel();
        assert!(run.as_mut().poll(&mut cx).is_pending());
        drop(owner);
        assert_eq!(run.as_mut().poll(&mut cx), Poll::Ready(None));
    }

    #[test]
    fn success_stub_maps_to_success_result_with_branch() {
        temper_worker_io::block_on(async {
            let outcome = StubExecutor::success().execute(assign("job-123")).await;
            let result = job_result("worker-1", "job-123", outcome);

            assert_eq!(result.protocol_version, WORKER_PROTOCOL_VERSION);
            assert_eq!(result.worker_id, "worker-1");
            assert_eq!(result.job_id, "job-123");
            assert_eq!(result.status, ResultStatus::Success);
            assert_eq!(result.failure, None);
            assert_eq!(result.details, None);
            assert_eq!(
                result.summary.as_deref(),
                Some("stub executor completed without doing IO")
            );
            assert_eq!(
                result.repos,
                vec![RepoOutcome {
                    repo: "ai/temper".to_string(),
                    branch: Branch {
                        name: "temper-worker/stub/job-123".to_string(),
                        head_sha: "0000000000000000000000000000000000000000".to_string(),
                    },
                }]
            );
        });
    }

    #[test]
    fn success_outcome_maps_structured_details_to_result() {
        let details = json!({"extra":{"note":"worker metadata"}});
        let result = job_result(
            "worker-1",
            "job-123",
            JobOutcome::Success {
                repos: Vec::new(),
                title: None,
                body: None,
                summary: Some("implemented".to_string()),
                details: Some(details.clone()),
            },
        );

        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(result.summary.as_deref(), Some("implemented"));
        assert_eq!(result.details, Some(details));
    }

    #[test]
    fn success_outcome_maps_handoff_title_and_body_to_result() {
        let result = job_result(
            "worker-1",
            "job-123",
            JobOutcome::Success {
                repos: Vec::new(),
                title: Some("Implement agent-authored handoff".to_string()),
                body: Some("# Implementation report\n\nDone.".to_string()),
                summary: Some("implemented".to_string()),
                details: None,
            },
        );

        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(
            result.title.as_deref(),
            Some("Implement agent-authored handoff")
        );
        assert_eq!(
            result.body.as_deref(),
            Some("# Implementation report\n\nDone.")
        );
        assert_eq!(result.verdict, None);
    }

    #[test]
    fn verdict_outcome_maps_to_success_result_without_branch() {
        let result = job_result(
            "worker-3",
            "job-789",
            JobOutcome::Verdict {
                verdict: "ready_code".to_string(),
                title: None,
                body: Some("rewritten issue body".to_string()),
                summary: Some("triaged".to_string()),
                children: Vec::new(),
                details: None,
            },
        );

        assert_eq!(result.protocol_version, WORKER_PROTOCOL_VERSION);
        assert_eq!(result.worker_id, "worker-3");
        assert_eq!(result.job_id, "job-789");
        assert_eq!(result.status, ResultStatus::Success);
        assert!(result.repos.is_empty());
        assert_eq!(result.failure, None);
        assert_eq!(result.summary.as_deref(), Some("triaged"));
        assert!(result.children.is_empty());

        let serialized = serde_json::to_value(&result).expect("JobResult serializes");
        assert_eq!(serialized["verdict"], "ready_code");
        assert_eq!(serialized["body"], "rewritten issue body");
        assert!(
            serialized.get("children").is_none(),
            "empty children must stay wire-compatible: {serialized}"
        );
    }

    #[test]
    fn verdict_outcome_preserves_authored_title_for_metadata_pr_create() {
        let result = job_result(
            "worker-3",
            "job-789",
            JobOutcome::Verdict {
                verdict: "passed".to_string(),
                title: Some("Land validated feature branch".to_string()),
                body: Some("# Validation report".to_string()),
                summary: Some("validated".to_string()),
                children: Vec::new(),
                details: None,
            },
        );

        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(result.verdict.as_deref(), Some("passed"));
        assert_eq!(
            result.title.as_deref(),
            Some("Land validated feature branch")
        );
    }

    #[test]
    fn verdict_outcome_maps_children_to_success_result() {
        let children = vec![
            JobChild {
                slug: "api-schema".to_string(),
                title: "Define the API schema".to_string(),
                body: "Write the shared API schema.".to_string(),
                kind: None,
                labels: vec!["code".to_string(), "ready".to_string()],
                depends_on: Vec::new(),
                target_repo: Some("acme/api".to_string()),
            },
            JobChild {
                slug: "web-client".to_string(),
                title: "Implement the web client".to_string(),
                body: "Build the web client against the API schema.".to_string(),
                kind: None,
                labels: vec!["code".to_string()],
                depends_on: vec!["api-schema".to_string()],
                target_repo: None,
            },
        ];

        let result = job_result(
            "worker-3",
            "job-789",
            JobOutcome::Verdict {
                verdict: "needs_breakdown".to_string(),
                title: None,
                body: None,
                summary: Some("planned breakdown".to_string()),
                children: children.clone(),
                details: None,
            },
        );

        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(result.children, children);

        let serialized = serde_json::to_value(&result).expect("JobResult serializes");
        assert_eq!(serialized["verdict"], "needs_breakdown");
        assert_eq!(serialized["children"][0]["slug"], "api-schema");
        assert_eq!(serialized["children"][0]["title"], "Define the API schema");
        assert_eq!(
            serialized["children"][0]["body"],
            "Write the shared API schema."
        );
        assert_eq!(
            serialized["children"][0]["labels"],
            json!(["code", "ready"])
        );
        assert_eq!(serialized["children"][0]["target_repo"], "acme/api");
        assert_eq!(serialized["children"][1]["slug"], "web-client");
        assert_eq!(
            serialized["children"][1]["depends_on"],
            json!(["api-schema"])
        );
        assert_eq!(serialized["children"][1]["labels"], json!(["code"]));
        assert!(serialized["children"][1].get("target_repo").is_none());
    }

    #[test]
    fn failure_outcome_preserves_typed_evidence_for_exact_attempt() {
        use temper_protocol_activity::{ModelFailureCategoryV1, ModelFailureV1};
        use temper_protocol_worker::{SessionRecoveryActionV1, SessionRecoveryEvidenceV1};

        let model_failure = ModelFailureV1 {
            provider: "openai-codex".to_string(),
            model: "gpt-safe".to_string(),
            category: ModelFailureCategoryV1::Response,
            disposition: temper_protocol_activity::ModelFailureDispositionV1::Retryable,
            boundary: temper_protocol_activity::ModelFailureBoundaryV1::Http,
            event_kind: temper_protocol_activity::ModelFailureEventKindV1::HttpResponse,
            status_present: true,
            code_present: true,
            retryable: true,
            http_status: Some(502),
            provider_request_id: Some("req_748".to_string()),
            provider_error_code: Some("malformed_stream".to_string()),
            message: "Provider returned a malformed stream.".to_string(),
            detail_redacted: false,
        };
        let session_recovery = SessionRecoveryEvidenceV1 {
            attempt_id: "attempt-748".to_string(),
            failure_epoch: 1,
            failure_count: 1,
            session_number: 0,
            session_failure_count: 0,
            epoch_started_unix_ms: None,
            epoch_elapsed_ms: 0,
            disposition: None,
            immediate_retry_exhausted: false,
            configured_session_failure_limit: 0,
            configured_fresh_session_limit: 0,
            configured_deferral_limit: 0,
            deferral_count: 0,
            deferral_generation: 0,
            not_before_unix_ms: None,
            slo_deadline_unix_ms: None,
            action: SessionRecoveryActionV1::RotateSession,
            current_session_id: "session-old".to_string(),
            prior_session_id: None,
            new_session_id: Some("session-new".to_string()),
            evidence_location: ".temper-agent-session/state.json".to_string(),
        };
        let result = job_result_for_attempt(
            "worker-1",
            "job-748",
            Some("attempt-748".to_string()),
            JobOutcome::Failure {
                class: FailureClass::Transient,
                message: "model failure".to_string(),
                model_failure: Some(model_failure.clone()),
                session_recovery: Some(session_recovery.clone()),
            },
        );

        let failure = result.failure.expect("failure evidence");
        assert_eq!(failure.model_failure, Some(model_failure));
        assert_eq!(failure.session_recovery, Some(session_recovery));
    }

    #[test]
    fn typed_recovery_action_normalizes_failure_class_without_text_authority() {
        let evidence: SessionRecoveryEvidenceV1 = serde_json::from_value(json!({
            "attempt_id": "attempt-deferred",
            "failure_epoch": 1,
            "failure_count": 2,
            "session_number": 2,
            "session_failure_count": 1,
            "epoch_started_unix_ms": 1000,
            "epoch_elapsed_ms": 100,
            "disposition": "unknown",
            "immediate_retry_exhausted": true,
            "configured_session_failure_limit": 1,
            "configured_fresh_session_limit": 1,
            "configured_deferral_limit": 3,
            "deferral_count": 1,
            "deferral_generation": 1,
            "not_before_unix_ms": 1200,
            "slo_deadline_unix_ms": 5000,
            "action": "provider_deferred",
            "current_session_id": "session-current",
            "prior_session_id": "session-prior",
            "evidence_location": ".temper-agent-session/state.json"
        }))
        .unwrap();
        let result = job_result_for_attempt(
            "worker-1",
            "job-deferred",
            Some("attempt-deferred".to_string()),
            JobOutcome::Failure {
                class: FailureClass::Permanent,
                message: "untrusted generic text says permanent".to_string(),
                model_failure: None,
                session_recovery: Some(evidence.clone()),
            },
        );
        assert_eq!(result.failure.unwrap().class, FailureClass::Transient);

        let known_failure: temper_protocol_activity::ModelFailureV1 =
            serde_json::from_value(json!({
                "provider": "fixture-provider",
                "model": "fixture-model",
                "category": "authentication",
                "retryable": false,
                "http_status": 401,
                "provider_error_code": "invalid_api_key",
                "message": "Provider authentication failed.",
                "detail_redacted": false
            }))
            .unwrap();
        let contradictory = job_result_for_attempt(
            "worker-1",
            "job-deferred",
            Some("attempt-deferred".to_string()),
            JobOutcome::Failure {
                class: FailureClass::Permanent,
                message: "typed diagnostic says actionable".to_string(),
                model_failure: Some(known_failure),
                session_recovery: Some(evidence),
            },
        );
        let contradictory = contradictory.failure.unwrap();
        assert_eq!(contradictory.class, FailureClass::Permanent);
        assert!(contradictory.session_recovery.is_none());
    }

    #[test]
    fn failure_stub_maps_to_failure_result_without_branch() {
        temper_worker_io::block_on(async {
            let outcome = StubExecutor::failure(FailureClass::Permanent, "configured failure")
                .execute(assign("job-456"))
                .await;
            let result = job_result("worker-2", "job-456", outcome);

            assert_eq!(result.protocol_version, WORKER_PROTOCOL_VERSION);
            assert_eq!(result.worker_id, "worker-2");
            assert_eq!(result.job_id, "job-456");
            assert_eq!(result.status, ResultStatus::Failure);
            assert!(result.repos.is_empty());
            assert_eq!(result.summary, None);
            assert_eq!(
                result.failure,
                Some(Failure {
                    class: FailureClass::Permanent,
                    message: "configured failure".to_string(),
                    model_failure: None,
                    session_recovery: None,
                })
            );
        });
    }
}
