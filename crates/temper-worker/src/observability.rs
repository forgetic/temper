//! Operational log-line formatting for the worker.
//!
//! Pure formatting helpers so the worker's observability contract is unit-tested
//! and the [`WorkerMachine`](crate::worker_machine::WorkerMachine) can emit log
//! lines as data ([`WorkerRequest::Log`](crate::worker_machine::WorkerRequest))
//! without doing I/O.

use temper_protocol_worker::{Assign, FailureClass, JobResult, ResultStatus};

use crate::config::CapabilitySpec;
mod containment;
mod liveness;
pub use containment::{
    CleanupBlocked, CleanupCompleted, ContainmentEvent, ContainmentEventContext,
    ContainmentEventIdentity, ContainmentEventObserver, ContainmentFallbackActivated,
    ContainmentStartupCapability, TracingContainmentEventObserver,
    observe_startup_containment_capability,
};
pub(crate) use containment::{ContainmentEventThrottle, emit_startup_containment_capability_once};
pub use liveness::{ObservedOperation, WorkerEvent};

pub fn registered_worker_line(
    worker_id: &str,
    worker_pool: Option<&str>,
    max_concurrent_jobs: u32,
    capabilities: &[CapabilitySpec],
) -> String {
    let capabilities = capability_list(capabilities);
    match worker_pool {
        Some(pool) => format!(
            "worker: registered worker_id={worker_id} pool={pool} capacity={max_concurrent_jobs} capabilities={capabilities}"
        ),
        None => format!(
            "worker: registered worker_id={worker_id} capacity={max_concurrent_jobs} capabilities={capabilities}"
        ),
    }
}

fn capability_list(capabilities: &[CapabilitySpec]) -> String {
    let values = capabilities
        .iter()
        .map(|capability| format!("{}:{}", capability.repo, capability.role))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{values}]")
}

pub fn assigned_job_line(assign: &Assign) -> String {
    format!(
        "worker: assigned job_id={} role={} repo={}",
        assign.job_id, assign.role, assign.repo
    )
}

pub fn result_sent_line(result: &JobResult) -> String {
    format!(
        "worker: result sent job_id={} status={}",
        result.job_id,
        result_status_display(result)
    )
}

fn result_status_display(result: &JobResult) -> String {
    match result.status {
        ResultStatus::Success => "success".to_string(),
        ResultStatus::Failure => {
            let class = result
                .failure
                .as_ref()
                .map(|failure| match failure.class {
                    FailureClass::Transient => "transient",
                    FailureClass::Permanent => "permanent",
                    FailureClass::Canceled => "canceled",
                    FailureClass::Protocol => "protocol",
                })
                .unwrap_or("unknown");
            format!("failure({class})")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use temper_protocol_worker::{Artifact, Branch, Failure, WORKER_PROTOCOL_VERSION};
    use tracing_subscriber::fmt::MakeWriter;

    use crate::config::CapabilitySpec;

    use super::*;

    fn assign() -> Assign {
        Assign {
            protocol_version: WORKER_PROTOCOL_VERSION,
            trace_context: None,
            job_id: "job-123".to_string(),
            attempt_id: Some("attempt-123".to_string()),
            role: "engineer".to_string(),
            repo: "acme/service".to_string(),
            artifact: Artifact {
                item: json!(1),
                kind: "issue".to_string(),
            },
            job_payload: json!({}),
        }
    }

    fn capabilities() -> Vec<CapabilitySpec> {
        vec![
            CapabilitySpec {
                repo: "ai/temper".to_string(),
                role: "engineer".to_string(),
            },
            CapabilitySpec {
                repo: "acme/service".to_string(),
                role: "reviewer".to_string(),
            },
        ]
    }

    #[test]
    fn registered_worker_line_matches_observability_contract() {
        let capabilities = capabilities();
        assert_eq!(
            registered_worker_line("basic-delivery-1", None, 2, &capabilities),
            "worker: registered worker_id=basic-delivery-1 capacity=2 capabilities=[ai/temper:engineer,acme/service:reviewer]"
        );
        assert_eq!(
            registered_worker_line("basic-delivery-1", Some("builders"), 2, &capabilities),
            "worker: registered worker_id=basic-delivery-1 pool=builders capacity=2 capabilities=[ai/temper:engineer,acme/service:reviewer]"
        );
    }

    #[test]
    fn assigned_job_line_matches_observability_contract() {
        assert_eq!(
            assigned_job_line(&assign()),
            "worker: assigned job_id=job-123 role=engineer repo=acme/service"
        );
    }

    #[test]
    fn result_sent_line_formats_success_status() {
        let result = test_job_result(json!({
            "protocol_version": WORKER_PROTOCOL_VERSION,
            "worker_id": "worker-1",
            "job_id": "job-123",
            "status": ResultStatus::Success,
            "branch": Branch {
                name: "agent/pr-for-code-1".to_string(),
                head_sha: "abc123".to_string(),
            },
            "failure": null,
            "verdict": null,
            "body": null,
            "summary": null,
            "details": null,
        }));

        assert_eq!(
            result_sent_line(&result),
            "worker: result sent job_id=job-123 status=success"
        );
    }

    #[test]
    fn result_sent_line_formats_failure_class() {
        let result = test_job_result(json!({
            "protocol_version": WORKER_PROTOCOL_VERSION,
            "worker_id": "worker-1",
            "job_id": "job-456",
            "status": ResultStatus::Failure,
            "branch": null,
            "failure": Failure {
                class: FailureClass::Permanent,
                message: "configured failure".to_string(),
            },
            "verdict": null,
            "body": null,
            "summary": null,
            "details": null,
        }));

        assert_eq!(
            result_sent_line(&result),
            "worker: result sent job_id=job-456 status=failure(permanent)"
        );
    }

    #[test]
    fn result_sent_line_formats_failure_without_details_as_unknown() {
        let result = test_job_result(json!({
            "protocol_version": WORKER_PROTOCOL_VERSION,
            "worker_id": "worker-1",
            "job_id": "job-789",
            "status": ResultStatus::Failure,
            "branch": null,
            "failure": null,
            "verdict": null,
            "body": null,
            "summary": null,
            "details": null,
        }));

        assert_eq!(
            result_sent_line(&result),
            "worker: result sent job_id=job-789 status=failure(unknown)"
        );
    }

    #[test]
    fn liveness_catalog_emits_structured_levels_without_sensitive_fields() {
        let events = vec![
            WorkerEvent::JobProgress {
                worker_id: "worker-1".into(),
                job_id: "job-1".into(),
                attempt_id: "attempt-1".into(),
                phase: "running",
                run_elapsed_ms: 10,
                last_progress_elapsed_ms: 0,
                no_progress_elapsed_ms: 0,
                active_parallel_operation_count: 1,
                operation: Some(ObservedOperation {
                    kind: "tool",
                    name: "forge_list_related".into(),
                    operation_id: "call-1".into(),
                    elapsed_ms: 3,
                }),
            },
            WorkerEvent::JobTimeout {
                worker_id: "worker-1".into(),
                job_id: "job-1".into(),
                attempt_id: "attempt-1".into(),
                phase: "cancel_requested",
                reason: "no_progress",
                limit_ms: 30_000,
                run_elapsed_ms: 90_000,
                last_progress_elapsed_ms: 30_001,
                no_progress_elapsed_ms: 30_001,
                active_parallel_operation_count: 1,
                operation: None,
            },
            WorkerEvent::CancellationRequested {
                worker_id: "worker-1".into(),
                job_id: "job-1".into(),
                attempt_id: "attempt-1".into(),
                reason: "no_progress",
                limit_ms: 30_000,
            },
            WorkerEvent::CancellationCompleted {
                worker_id: "worker-1".into(),
                job_id: "job-1".into(),
                attempt_id: "attempt-1".into(),
                outcome: "hard_kill".into(),
                descendant_cleanup: "hard_killed".into(),
                forced: true,
            },
            WorkerEvent::ResultRecorded {
                worker_id: "worker-1".into(),
                job_id: "job-1".into(),
                attempt_id: "attempt-1".into(),
                outbox_state: "durable",
                delivery_state: "pending",
                success: true,
            },
            WorkerEvent::ResultDelivery {
                worker_id: "worker-1".into(),
                job_id: "job-1".into(),
                attempt_id: "attempt-1".into(),
                outbox_state: "durable",
                delivery_state: "retrying",
                claim_convergence: "pending",
                warning: true,
            },
            WorkerEvent::CapacityReleased {
                worker_id: "worker-1".into(),
                job_id: "job-1".into(),
                attempt_id: "attempt-1".into(),
                permit_released: true,
                free_capacity: 1,
            },
        ];
        let captured = capture_events(|| {
            for event in &events {
                event.emit();
            }
        });
        assert_eq!(captured.len(), events.len());
        for (captured, expected) in captured.iter().zip(events.iter()) {
            assert_eq!(captured["fields"]["event"], expected.name());
            assert_eq!(captured["fields"]["worker_id"], "worker-1");
            assert_eq!(captured["fields"]["job_id"], "job-1");
            assert_eq!(captured["fields"]["attempt_id"], "attempt-1");
        }
        assert_eq!(captured[0]["level"], "DEBUG");
        assert_eq!(captured[1]["level"], "WARN");
        assert_eq!(captured[2]["level"], "WARN");
        assert_eq!(captured[3]["level"], "WARN");
        assert_eq!(captured[4]["level"], "DEBUG");
        assert_eq!(captured[5]["level"], "WARN");
        assert_eq!(captured[6]["level"], "DEBUG");
        let encoded = serde_json::to_string(&captured).unwrap();
        for forbidden in [
            "tool_arguments",
            "result_body",
            "prompt_content",
            "credentials",
            "secret-token-sentinel",
        ] {
            assert!(!encoded.contains(forbidden), "event leaked {forbidden}");
        }
    }

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for SharedBuffer {
        type Writer = SharedBuffer;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture_events(run: impl FnOnce()) -> Vec<serde_json::Value> {
        let buffer = SharedBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(buffer.clone())
            .with_max_level(tracing::Level::TRACE)
            .finish();
        tracing::subscriber::with_default(subscriber, run);
        let bytes = buffer.0.lock().unwrap().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn test_job_result(value: serde_json::Value) -> JobResult {
        serde_json::from_value(value).expect("test JobResult JSON matches worker protocol")
    }
}
