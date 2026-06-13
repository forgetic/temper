//! Operational log-line formatting for the worker.
//!
//! Pure formatting helpers so the worker's observability contract is unit-tested
//! and the [`WorkerMachine`](crate::worker_machine::WorkerMachine) can emit log
//! lines as data ([`WorkerRequest::Log`](crate::worker_machine::WorkerRequest))
//! without doing I/O.

use smith_agent_protocol::StepProgress;
use temper_worker_protocol::{Assign, FailureClass, JobResult, ResultStatus};

pub fn registered_worker_line(worker_id: &str, capability_count: usize) -> String {
    format!("smith-worker: registered worker_id={worker_id} capabilities={capability_count}")
}

/// One operational line per agent step-progress checkpoint the worker relays.
/// The `correlation_key` joins it to the job (and to the out-of-band control
/// plane); `sha` is the pushed checkpoint, when present, for crash recovery.
pub fn step_progress_line(progress: &StepProgress) -> String {
    let sha = progress.pushed_sha.as_deref().unwrap_or("-");
    format!(
        "smith-worker: step-progress correlation={} step={} status={:?} sha={} :: {}",
        progress.correlation_key, progress.step, progress.state, sha, progress.status
    )
}

pub fn assigned_job_line(assign: &Assign) -> String {
    format!(
        "smith-worker: assigned job_id={} role={} repo={}",
        assign.job_id, assign.role, assign.repo
    )
}

pub fn result_sent_line(result: &JobResult) -> String {
    format!(
        "smith-worker: result sent job_id={} status={}",
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
    use temper_worker_protocol::{Artifact, Branch, Failure, WORKER_PROTOCOL_VERSION};

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

    #[test]
    fn registered_worker_line_matches_observability_contract() {
        assert_eq!(
            registered_worker_line("basic-delivery-1", 2),
            "smith-worker: registered worker_id=basic-delivery-1 capabilities=2"
        );
    }

    #[test]
    fn assigned_job_line_matches_observability_contract() {
        assert_eq!(
            assigned_job_line(&assign()),
            "smith-worker: assigned job_id=job-123 role=engineer repo=acme/service"
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
            "smith-worker: result sent job_id=job-123 status=success"
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
            "smith-worker: result sent job_id=job-456 status=failure(permanent)"
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
            "smith-worker: result sent job_id=job-789 status=failure(unknown)"
        );
    }

    fn test_job_result(value: serde_json::Value) -> JobResult {
        serde_json::from_value(value).expect("test JobResult JSON matches worker protocol")
    }
}
