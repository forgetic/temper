// SPDX-License-Identifier: MPL-2.0

//! Worker-protocol encoding helpers and structured log lines shared by the
//! daemon machine and its handlers.

use temper_engine_io::http::HttpResponseData;
use temper_worker_protocol::{
    Assign, ErrorCode, FailureClass, JobProgress, JobResult, ResultStatus, WorkerProtocolMessage,
};

use crate::InFlightJob;

pub(super) fn is_poll_timeout(message: &WorkerProtocolMessage) -> bool {
    matches!(
        message,
        WorkerProtocolMessage::Error(error) if error.code == ErrorCode::PollTimeout
    )
}

/// Decode the daemon's in-process HTTP response into the worker-protocol reply,
/// matching `crate::transport::Transport`'s contract: `Ok(None)` for 204/empty,
/// `Ok(Some(message))` for a 200 JSON body, `Err` otherwise. (Mirrors the
/// HTTP transport's `decode_reply` so the in-process and HTTP carriers agree.)
pub(super) fn decode_in_process_reply(
    response: HttpResponseData,
) -> Result<Option<WorkerProtocolMessage>, String> {
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
                        "daemon in-process reply was not valid worker protocol JSON: {error}; body: {body}"
                    )
                })
        }
        status => {
            let body = String::from_utf8_lossy(&response.body);
            Err(format!("daemon in-process reply HTTP {status}: {body}"))
        }
    }
}

pub(super) fn assignment_log_line(assign: &Assign, worker_id: &str) -> String {
    format!(
        "engine: assigned job_id={} role={} repo={} worker={}",
        assign.job_id, assign.role, assign.repo, worker_id
    )
}

pub(super) fn result_received_log_line(result: &JobResult, disposition: &str) -> String {
    format!(
        "engine: result received job_id={} worker={} status={} disposition={}",
        result.job_id,
        result.worker_id,
        result_status_log_value(result),
        disposition
    )
}

fn result_status_log_value(result: &JobResult) -> String {
    match result.status {
        ResultStatus::Success => "success".to_string(),
        ResultStatus::Failure => {
            let class = result
                .failure
                .as_ref()
                .map(|failure| failure_class_log_value(failure.class))
                .unwrap_or("unknown");
            format!("failure({class})")
        }
    }
}

fn failure_class_log_value(class: FailureClass) -> &'static str {
    match class {
        FailureClass::Transient => "transient",
        FailureClass::Permanent => "permanent",
        FailureClass::Canceled => "canceled",
        FailureClass::Protocol => "protocol",
    }
}

pub(super) fn result_disposition_log_value(disposition: ResultDisposition) -> &'static str {
    match disposition {
        ResultDisposition::Apply => "apply",
        ResultDisposition::DropForRescan => "rescan",
        ResultDisposition::Drop => "drop",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResultDisposition {
    Apply,
    DropForRescan,
    Drop,
}

pub(super) fn result_disposition(result: &JobResult) -> ResultDisposition {
    match result.status {
        ResultStatus::Success => ResultDisposition::Apply,
        ResultStatus::Failure => match result.failure.as_ref().map(|failure| failure.class) {
            Some(FailureClass::Transient) => ResultDisposition::DropForRescan,
            Some(FailureClass::Canceled) => ResultDisposition::Drop,
            Some(FailureClass::Permanent | FailureClass::Protocol) | None => {
                ResultDisposition::Apply
            }
        },
    }
}

/// One structured log line per accepted progress checkpoint.
pub(super) fn progress_log_line(job: &InFlightJob, progress: &JobProgress) -> String {
    format!(
        "engine: progress job_id={} correlation_key={} step={} state={} sha={}{} :: {}",
        job.job_id,
        progress.correlation_key,
        progress.step,
        progress.state,
        progress.pushed_sha.as_deref().unwrap_or("-"),
        progress
            .note
            .as_deref()
            .map(|note| format!(" note={note:?}"))
            .unwrap_or_default(),
        progress.status,
    )
}

/// Renders a worker-protocol core response as an HTTP response: `200` with a
/// JSON body, or `204` when the core had nothing to say.
pub(super) fn protocol_response(message: Option<WorkerProtocolMessage>) -> HttpResponseData {
    match message {
        Some(message) => HttpResponseData::json(
            200,
            &serde_json::to_value(&message).expect("protocol messages serialize"),
        ),
        None => HttpResponseData::status_only(204),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use temper_worker_protocol::{Artifact, Failure, WORKER_PROTOCOL_VERSION};

    fn result_for_disposition(
        status: ResultStatus,
        failure_class: Option<FailureClass>,
    ) -> JobResult {
        JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-a".to_string(),
            job_id: "job-1".to_string(),
            status,
            repos: Vec::new(),
            verdict: None,
            body: None,
            children: Vec::new(),
            failure: failure_class.map(|class| Failure {
                class,
                message: "worker failed".to_string(),
            }),
            summary: None,
            details: None,
        }
    }

    fn assign_for_log_line() -> Assign {
        Assign {
            protocol_version: WORKER_PROTOCOL_VERSION,
            job_id: "ai/temper/issue-147/engineer/code_ready".to_string(),
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            artifact: Artifact {
                item: json!(147),
                kind: "issue".to_string(),
            },
            job_payload: json!({"safe": "context"}),
        }
    }

    #[test]
    fn assignment_log_line_includes_worker_from_poll() {
        assert_eq!(
            assignment_log_line(&assign_for_log_line(), "worker-a"),
            "engine: assigned job_id=ai/temper/issue-147/engineer/code_ready role=engineer repo=ai/temper worker=worker-a"
        );
    }

    #[test]
    fn result_received_log_line_formats_success_status() {
        let result = result_for_disposition(ResultStatus::Success, None);

        assert_eq!(
            result_received_log_line(&result, "apply"),
            "engine: result received job_id=job-1 worker=worker-a status=success disposition=apply"
        );
    }

    #[test]
    fn result_received_log_line_formats_each_failure_class() {
        let cases = [
            (FailureClass::Transient, "transient", "rescan"),
            (FailureClass::Permanent, "permanent", "apply"),
            (FailureClass::Canceled, "canceled", "drop"),
            (FailureClass::Protocol, "protocol", "apply"),
        ];

        for (class, expected_class, disposition) in cases {
            let result = result_for_disposition(ResultStatus::Failure, Some(class));

            assert_eq!(
                result_received_log_line(&result, disposition),
                format!(
                    "engine: result received job_id=job-1 worker=worker-a status=failure({expected_class}) disposition={disposition}"
                )
            );
        }
    }

    #[test]
    fn result_disposition_routes_success_to_apply() {
        assert_eq!(
            result_disposition(&result_for_disposition(ResultStatus::Success, None)),
            ResultDisposition::Apply
        );
    }

    #[test]
    fn result_disposition_routes_transient_failure_to_drop_for_rescan() {
        assert_eq!(
            result_disposition(&result_for_disposition(
                ResultStatus::Failure,
                Some(FailureClass::Transient),
            )),
            ResultDisposition::DropForRescan
        );
    }

    #[test]
    fn result_disposition_routes_permanent_failure_to_apply() {
        assert_eq!(
            result_disposition(&result_for_disposition(
                ResultStatus::Failure,
                Some(FailureClass::Permanent),
            )),
            ResultDisposition::Apply
        );
    }

    #[test]
    fn result_disposition_routes_protocol_failure_to_apply() {
        assert_eq!(
            result_disposition(&result_for_disposition(
                ResultStatus::Failure,
                Some(FailureClass::Protocol),
            )),
            ResultDisposition::Apply
        );
    }

    #[test]
    fn result_disposition_routes_canceled_failure_to_drop() {
        assert_eq!(
            result_disposition(&result_for_disposition(
                ResultStatus::Failure,
                Some(FailureClass::Canceled),
            )),
            ResultDisposition::Drop
        );
    }

    #[test]
    fn result_disposition_routes_failure_without_details_to_apply() {
        assert_eq!(
            result_disposition(&result_for_disposition(ResultStatus::Failure, None)),
            ResultDisposition::Apply
        );
    }
}
