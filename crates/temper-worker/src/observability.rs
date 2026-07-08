//! Operational log-line formatting for the worker.
//!
//! Pure formatting helpers so the worker's observability contract is unit-tested
//! and the [`WorkerMachine`](crate::worker_machine::WorkerMachine) can emit log
//! lines as data ([`WorkerRequest::Log`](crate::worker_machine::WorkerRequest))
//! without doing I/O.

use temper_protocol_worker::{Assign, FailureClass, JobResult, ResultStatus};

use crate::config::CapabilitySpec;

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
    use serde_json::json;
    use temper_protocol_worker::{Artifact, Branch, Failure, WORKER_PROTOCOL_VERSION};

    use crate::config::CapabilitySpec;

    use super::*;

    fn assign() -> Assign {
        Assign {
            protocol_version: WORKER_PROTOCOL_VERSION,
            job_id: "job-123".to_string(),
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

    fn test_job_result(value: serde_json::Value) -> JobResult {
        serde_json::from_value(value).expect("test JobResult JSON matches worker protocol")
    }
}
