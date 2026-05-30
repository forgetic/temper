// SPDX-License-Identifier: MPL-2.0
//! Tests for Forgejo CI (Actions) job listing and lookup.
mod support;

use harness_forge::{
    CiConclusion, CiJobId, CiJobQuery, CiJobSort, CiStatus, PullRequestId, RepositoryId,
};

use harness_forge_forgejo::{ForgejoConfig, ForgejoForge};

use serde_json::json;

use support::MockHttpClient;

fn forge(client: MockHttpClient) -> ForgejoForge<MockHttpClient> {
    let config = ForgejoConfig::new("https://forge.example/api/v1");
    ForgejoForge::new(config, client)
}

fn repo() -> RepositoryId {
    RepositoryId::new("octo/demo")
}

fn task(id: i64, run_number: i64, name: &str, status: &str) -> serde_json::Value {
    json!({
        "id": id,
        "run_number": run_number,
        "name": name,
        "status": status,
        "head_sha": "abcdef1234567",
        "html_url": "https://forge.example/octo/demo/actions/runs/10/jobs/0",
        "created_at": "2024-01-02T03:04:05Z",
    })
}

#[tokio::test]
async fn list_by_pull_request_matches_ref_and_returns_latest_attempt() {
    let client = MockHttpClient::default();
    // PR detail (head ref/sha resolution).
    client.push_json(
        200,
        json!({ "number": 7, "head": { "ref": "feature", "sha": "abcdef1234567" } }),
    );
    // Runs (object-wrapped shape): run #7 matches; the push run does not.
    client.push_json(
        200,
        json!({
            "workflow_runs": [
                {
                    "index_in_repo": 10,
                    "run_number": 10,
                    "status": "success",
                    "event": "pull_request",
                    "prettyref": "#7",
                    "head_branch": "feature",
                    "head_sha": "abcdef1234567",
                    "html_url": "https://forge.example/octo/demo/actions/runs/10",
                    "created_at": "2024-01-02T00:00:00Z"
                },
                {
                    "index_in_repo": 11,
                    "run_number": 11,
                    "status": "success",
                    "event": "push",
                    "prettyref": "#8",
                    "head_branch": "other",
                    "head_sha": "9999999999",
                    "created_at": "2024-01-03T00:00:00Z"
                }
            ]
        }),
    );
    // Tasks: two attempts for run 10, plus an unrelated run 11 task.
    client.push_json(
        200,
        json!({
            "workflow_runs": [
                task(1, 10, "build", "success"),
                task(2, 10, "test", "success"),
                task(3, 10, "build", "success"),
                task(4, 10, "test", "failure"),
                task(5, 11, "lint", "success")
            ]
        }),
    );

    let forge = forge(client);
    let query = CiJobQuery {
        pull_request_id: Some(PullRequestId::new("7")),
        commit_sha: None,
        status: None,
        sort: None,
    };
    let jobs = forge.list_ci_jobs(&repo(), &query).await.unwrap();

    // Only the latest attempt of the matched run is returned.
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].name, "build");
    assert_eq!(jobs[0].status, CiStatus::Completed);
    assert_eq!(jobs[0].conclusion, Some(CiConclusion::Success));
    assert_eq!(jobs[1].name, "test");
    assert_eq!(jobs[1].conclusion, Some(CiConclusion::Failure));
    // The matched task ids come from the latest attempt (3, 4), not (1, 2).
    assert_eq!(jobs[0].id.as_str(), "run/10/task/3/job/0");
    assert_eq!(jobs[1].id.as_str(), "run/10/task/4/job/1");
    // Pull request and commit are carried onto each job.
    assert_eq!(jobs[0].pull_request_id, Some(PullRequestId::new("7")));
    assert_eq!(jobs[0].commit_sha.as_deref(), Some("abcdef1234567"));
}

#[tokio::test]
async fn list_by_commit_filters_status_and_sorts_by_name() {
    let client = MockHttpClient::default();
    // No PR detail call: the query targets a commit only.
    client.push_json(
        200,
        json!({
            "workflow_runs": [
                {
                    "index_in_repo": 20,
                    "run_number": 20,
                    "status": "success",
                    "event": "push",
                    "prettyref": "main",
                    "head_branch": "main",
                    "head_sha": "abcdef1234567",
                    "created_at": "2024-02-01T00:00:00Z"
                }
            ]
        }),
    );
    client.push_json(
        200,
        json!({
            "workflow_runs": [
                task(1, 20, "zebra", "success"),
                task(2, 20, "alpha", "failure"),
                task(3, 20, "mid", "running")
            ]
        }),
    );

    let forge = forge(client);
    let query = CiJobQuery {
        pull_request_id: None,
        commit_sha: Some("abcdef1234567".to_string()),
        status: Some(CiStatus::Completed),
        sort: Some(CiJobSort::Name),
    };
    let jobs = forge.list_ci_jobs(&repo(), &query).await.unwrap();

    // The running task is filtered out; the rest are sorted by name.
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].name, "alpha");
    assert_eq!(jobs[0].conclusion, Some(CiConclusion::Failure));
    assert_eq!(jobs[1].name, "zebra");
    assert_eq!(jobs[1].conclusion, Some(CiConclusion::Success));
    assert_eq!(jobs[0].commit_sha.as_deref(), Some("abcdef1234567"));
    // No pull request could be derived from a push run.
    assert_eq!(jobs[0].pull_request_id, None);
}

#[tokio::test]
async fn get_ci_job_parses_id_and_returns_job() {
    let client = MockHttpClient::default();
    client.push_json(
        200,
        json!({
            "workflow_runs": [
                {
                    "index_in_repo": 30,
                    "run_number": 30,
                    "status": "success",
                    "event": "push",
                    "head_branch": "main",
                    "head_sha": "abcdef1234567",
                    "created_at": "2024-03-01T00:00:00Z"
                }
            ]
        }),
    );
    client.push_json(
        200,
        json!({ "workflow_runs": [ task(1, 30, "build", "success") ] }),
    );

    let forge = forge(client);
    let id = CiJobId("run/30/task/1/job/0".to_string());
    let job = forge.get_ci_job(&repo(), &id).await.unwrap().unwrap();
    assert_eq!(job.id.as_str(), "run/30/task/1/job/0");
    assert_eq!(job.name, "build");
    assert_eq!(job.status, CiStatus::Completed);
    assert_eq!(job.conclusion, Some(CiConclusion::Success));
}

#[tokio::test]
async fn actions_unavailable_is_an_error() {
    let client = MockHttpClient::default();
    // The runs endpoint is unavailable: CI must surface an error, never "passed".
    client.push_json(404, json!({ "message": "Not Found" }));

    let forge = forge(client);
    let query = CiJobQuery {
        pull_request_id: None,
        commit_sha: None,
        status: None,
        sort: None,
    };
    let result = forge.list_ci_jobs(&repo(), &query).await;
    assert!(result.is_err());
}
