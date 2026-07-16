// SPDX-License-Identifier: MPL-2.0

use crate::{Branch, JobChild, JobResult, RepoOutcome, ResultStatus, WORKER_PROTOCOL_VERSION};

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
