// SPDX-License-Identifier: MPL-2.0

use crate::{
    Branch, JobChild, JobResult, RepoOutcome, ResultStatus, SessionRecoveryActionV1,
    WORKER_PROTOCOL_VERSION,
};

#[test]
fn coordinated_job_result_round_trips_with_multiple_repos() {
    let result = JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-1".to_string(),
        job_id: "job-123".to_string(),
        attempt_id: Some("attempt-123".to_string()),
        status: ResultStatus::Success,
        repos: vec![
            RepoOutcome {
                repo: "ai/temper".to_string(),
                branch: Branch {
                    name: "agent/coord-for-code-42".to_string(),
                    head_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
                },
            },
            RepoOutcome {
                repo: "ai/smith".to_string(),
                branch: Branch {
                    name: "agent/coord-for-code-42".to_string(),
                    head_sha: "89abcdef0123456789abcdef0123456789abcdef".to_string(),
                },
            },
        ],
        verdict: None,
        title: Some("Implement coordinated workspace changes".to_string()),
        body: Some("# Implementation report\n\nUpdated both repositories.".to_string()),
        children: Vec::new(),
        failure: None,
        summary: Some("implemented coord-for-code-42 across 2 repos".to_string()),
        details: None,
    };

    let value = serde_json::to_value(&result).expect("job result serializes");
    assert_eq!(value["repos"][0]["repo"], "ai/temper");
    assert_eq!(
        value["repos"][1]["branch"]["name"],
        "agent/coord-for-code-42"
    );
    assert_eq!(value.get("verdict"), None);
    assert_eq!(value["title"], "Implement coordinated workspace changes");
    assert_eq!(
        value["body"],
        "# Implementation report\n\nUpdated both repositories."
    );
    let decoded: JobResult = serde_json::from_value(value).expect("serialized result parses");
    assert_eq!(decoded, result);
}

#[test]
fn verdict_job_result_round_trips_without_repos() {
    let result = JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-1".to_string(),
        job_id: "job-123".to_string(),
        attempt_id: Some("attempt-123".to_string()),
        status: ResultStatus::Success,
        repos: Vec::new(),
        verdict: Some("ready_code".to_string()),
        title: None,
        body: Some("Rewritten implementation-ready issue body.".to_string()),
        children: Vec::new(),
        failure: None,
        summary: Some("triaged intake".to_string()),
        details: None,
    };

    let value = serde_json::to_value(&result).expect("job result serializes");
    assert_eq!(value.get("repos"), None);
    assert_eq!(value.get("children"), None);
    assert_eq!(value["verdict"], "ready_code");
    let decoded: JobResult = serde_json::from_value(value).expect("serialized result parses");
    assert_eq!(decoded, result);
}

#[test]
fn verdict_job_result_round_trips_with_children() {
    let result = JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-1".to_string(),
        job_id: "job-123".to_string(),
        attempt_id: Some("attempt-123".to_string()),
        status: ResultStatus::Success,
        repos: Vec::new(),
        verdict: Some("needs_breakdown".to_string()),
        title: None,
        body: None,
        children: vec![
            JobChild {
                slug: "api-schema".to_string(),
                title: "Define the API schema".to_string(),
                body: "Write the shared API schema.".to_string(),
                kind: None,
                labels: vec!["code".to_string(), "ready".to_string()],
                depends_on: Vec::new(),
                target_repo: None,
            },
            JobChild {
                slug: "web-client".to_string(),
                title: "Implement the web client".to_string(),
                body: "Build the web client against the API schema.".to_string(),
                kind: None,
                labels: vec!["code".to_string(), "ready".to_string()],
                depends_on: vec!["api-schema".to_string()],
                target_repo: Some("acme/web".to_string()),
            },
        ],
        failure: None,
        summary: Some("planned breakdown".to_string()),
        details: None,
    };

    let value = serde_json::to_value(&result).expect("job result serializes");
    assert_eq!(value.get("repos"), None);
    assert_eq!(value["verdict"], "needs_breakdown");
    assert_eq!(value["children"][1]["target_repo"], "acme/web");
    let decoded: JobResult = serde_json::from_value(value).expect("serialized result parses");
    assert_eq!(decoded, result);
}

#[test]
fn legacy_failure_json_stays_compatible() {
    let result: JobResult = serde_json::from_value(serde_json::json!({
        "protocol_version": 1,
        "worker_id": "worker-1",
        "job_id": "job-legacy",
        "attempt_id": "attempt-legacy",
        "status": "failure",
        "failure": {"class": "transient", "message": "legacy failure"}
    }))
    .expect("legacy failure parses");
    let failure = result.failure.expect("failure present");
    assert_eq!(failure.model_failure, None);
    assert_eq!(failure.session_recovery, None);
}

#[test]
fn typed_model_and_session_failure_evidence_round_trips() {
    let raw = serde_json::json!({
        "protocol_version": 1,
        "worker_id": "worker-1",
        "job_id": "job-748",
        "attempt_id": "attempt-748",
        "status": "failure",
        "failure": {
            "class": "transient",
            "message": "model call failed",
            "model_failure": {
                "provider": "openai-codex",
                "model": "gpt-safe",
                "category": "rate_limit",
                "disposition": "retryable",
                "boundary": "http",
                "event_kind": "http_response",
                "status_present": true,
                "code_present": true,
                "retryable": true,
                "http_status": 429,
                "provider_request_id": "req_748",
                "provider_error_code": "rate_limit",
                "message": "Provider rate limit reached.",
                "detail_redacted": false
            },
            "session_recovery": {
                "attempt_id": "attempt-748",
                "failure_epoch": 2,
                "failure_count": 3,
                "action": "rotate_session",
                "current_session_id": "session-current",
                "prior_session_id": "session-prior",
                "new_session_id": "session-new",
                "evidence_location": ".temper-agent-session/state.json"
            }
        }
    });
    let mut result: JobResult = serde_json::from_value(raw.clone()).expect("typed failure parses");
    result.normalize_failure_evidence();
    let failure = result.failure.as_ref().unwrap();
    failure.model_failure.as_ref().unwrap().validate().unwrap();
    let recovery = failure.session_recovery.as_ref().unwrap();
    recovery
        .validate_for_attempt(result.attempt_id.as_deref())
        .unwrap();
    assert_eq!(recovery.action, SessionRecoveryActionV1::RotateSession);
    assert_eq!(serde_json::to_value(result).unwrap(), raw);
}

#[test]
fn malformed_or_mismatched_session_evidence_is_not_retained() {
    let mut result: JobResult = serde_json::from_value(serde_json::json!({
        "protocol_version": 1,
        "worker_id": "worker-1",
        "job_id": "job-748",
        "attempt_id": "attempt-748",
        "status": "failure",
        "failure": {
            "class": "transient",
            "message": "failed",
            "session_recovery": {
                "attempt_id": "attempt-748",
                "failure_epoch": 1,
                "failure_count": 1,
                "action": "park_for_human",
                "current_session_id": "session-current",
                "evidence_location": "authorization:Bearer-secret"
            }
        }
    }))
    .expect("wire DTO remains additively readable");
    result.normalize_failure_evidence();
    assert!(result.failure.unwrap().session_recovery.is_none());
}

#[test]
fn child_defaults_omit_empty_optional_fields() {
    let child = JobChild {
        slug: "api-schema".to_string(),
        title: "Define the API schema".to_string(),
        body: "Write the shared API schema.".to_string(),
        kind: None,
        labels: Vec::new(),
        depends_on: Vec::new(),
        target_repo: None,
    };

    assert_eq!(
        serde_json::to_value(&child).expect("child serializes"),
        serde_json::json!({
            "slug": "api-schema",
            "title": "Define the API schema",
            "body": "Write the shared API schema."
        })
    );
}
