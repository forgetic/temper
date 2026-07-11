// SPDX-License-Identifier: MPL-2.0
//! Offline tests for Forgejo CI (Actions) job listing and lookup.
//!
//! These drive the backend through the recording mock HTTP client with canned
//! Actions runs/tasks JSON. No test touches the network.
mod support;

use serde_json::json;
use support::{MockHttpClient, block_on, forge, pull_id, repo_id};
use temper_forge_model::{
    CiJobConclusion, CiJobId, CiJobQuery, CiJobSort, CiJobSortField, CiJobStatus, SortDirection,
};

/// Builds a task JSON object with the fields the adapter consumes.
fn task(id: u64, run_number: u64, name: &str, status: &str) -> serde_json::Value {
    task_with_head(id, run_number, name, status, "abcdef1234567")
}

fn task_with_head(
    id: u64,
    run_number: u64,
    name: &str,
    status: &str,
    head_sha: &str,
) -> serde_json::Value {
    json!({
        "id": id,
        "run_number": run_number,
        "name": name,
        "status": status,
        "head_sha": head_sha,
        "html_url": "https://forge.example.com/acme/widgets/actions/runs/10/jobs/0",
        "created_at": "2024-01-02T03:04:05Z",
    })
}

#[test]
fn list_by_pull_request_matches_ref_and_returns_latest_attempt() {
    let client = MockHttpClient::new();
    // 1) PR detail (head ref/sha resolution).
    client.push_response(
        200,
        json!({
            "number": 7,
            "state": "open",
            "user": { "login": "author" },
            "head": { "ref": "feature", "sha": "abcdef1234567" },
            "base": { "ref": "main" },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z"
        })
        .to_string(),
    );
    // 2) Runs (object-wrapped): run #7 matches; the push run does not.
    client.push_response(
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
                    "html_url": "https://forge.example.com/acme/widgets/actions/runs/10",
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
        })
        .to_string(),
    );
    // 3) Tasks: two attempts for run 10, plus an unrelated run 11 task.
    client.push_response(
        200,
        json!({
            "workflow_runs": [
                task(1, 10, "build", "success"),
                task(2, 10, "test", "success"),
                task(3, 10, "build", "success"),
                task(4, 10, "test", "failure"),
                task(5, 11, "lint", "success")
            ]
        })
        .to_string(),
    );

    let forge = forge(client);
    let query = CiJobQuery {
        pull_request_id: Some(pull_id(7)),
        ..Default::default()
    };
    let jobs = block_on(forge.list_ci_jobs(&repo_id(), query)).unwrap();

    // Only the latest attempt of the matched run is returned, sorted by name.
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].name, "build");
    assert_eq!(jobs[0].status, CiJobStatus::Completed);
    assert_eq!(jobs[0].conclusion, Some(CiJobConclusion::Success));
    assert_eq!(jobs[1].name, "test");
    assert_eq!(jobs[1].conclusion, Some(CiJobConclusion::Failure));
    // The encoded ids come from the latest attempt (task ids 3, 4).
    assert_eq!(jobs[0].id.as_str(), "forgejo:acme/widgets:actions:10:0:3");
    assert_eq!(jobs[1].id.as_str(), "forgejo:acme/widgets:actions:10:1:4");
    // Pull request and commit are carried onto each job.
    assert_eq!(jobs[0].pull_request_id, Some(pull_id(7)));
    assert_eq!(jobs[0].repo_id, repo_id());
    assert_eq!(jobs[0].commit_sha, "abcdef1234567");
}

#[test]
fn list_by_pull_request_keeps_prior_push_run_on_same_head_branch() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        json!({
            "number": 7,
            "state": "open",
            "user": { "login": "author" },
            "head": { "ref": "feature", "sha": "newhead1234567" },
            "base": { "ref": "main" },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z"
        })
        .to_string(),
    );
    client.push_response(
        200,
        json!({
            "workflow_runs": [
                {
                    "index_in_repo": 2,
                    "run_number": 2,
                    "status": "success",
                    "event": "push",
                    "prettyref": "feature",
                    "head_branch": "",
                    "head_sha": "newhead1234567",
                    "created_at": "2024-01-03T00:00:00Z"
                },
                {
                    "index_in_repo": 1,
                    "run_number": 1,
                    "status": "failure",
                    "event": "push",
                    "prettyref": "feature",
                    "head_branch": "",
                    "head_sha": "oldhead1234567",
                    "created_at": "2024-01-02T00:00:00Z"
                }
            ]
        })
        .to_string(),
    );
    client.push_response(
        200,
        json!({
            "workflow_runs": [
                task_with_head(1, 1, "build", "failure", "oldhead1234567"),
                task_with_head(2, 2, "build", "success", "newhead1234567")
            ]
        })
        .to_string(),
    );

    let forge = forge(client);
    let jobs = block_on(forge.list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            pull_request_id: Some(pull_id(7)),
            status: Some(CiJobStatus::Completed),
            ..Default::default()
        },
    ))
    .unwrap();

    let conclusions: Vec<_> = jobs.iter().map(|job| job.conclusion).collect();
    assert_eq!(
        conclusions,
        vec![
            Some(CiJobConclusion::Failure),
            Some(CiJobConclusion::Success)
        ]
    );
    assert_eq!(jobs[0].commit_sha, "oldhead1234567");
    assert_eq!(jobs[1].commit_sha, "newhead1234567");
}

#[test]
fn combined_pr_and_commit_returns_only_current_queued_rest_jobs() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        json!({
            "number": 7,
            "state": "open",
            "user": { "login": "author" },
            "head": { "ref": "feature", "sha": "bbbbbbb2222222" },
            "base": { "ref": "main" },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z"
        })
        .to_string(),
    );
    client.push_response(
        200,
        json!({
            "workflow_runs": [
                {
                    "index_in_repo": 2,
                    "run_number": 2,
                    "status": "queued",
                    "event": "pull_request",
                    "prettyref": "#7",
                    "head_branch": "feature",
                    "head_sha": "bbbbbbb2222222",
                    "created_at": "2024-01-03T00:00:00Z"
                },
                {
                    "index_in_repo": 1,
                    "run_number": 1,
                    "status": "success",
                    "event": "pull_request",
                    "prettyref": "#7",
                    "head_branch": "feature",
                    "head_sha": "aaaaaaa1111111",
                    "created_at": "2024-01-02T00:00:00Z"
                }
            ]
        })
        .to_string(),
    );
    client.push_response(
        200,
        json!({
            "workflow_runs": [
                task_with_head(1, 1, "build", "success", "aaaaaaa1111111"),
                task_with_head(2, 2, "build", "queued", "bbbbbbb2222222")
            ]
        })
        .to_string(),
    );

    let jobs = block_on(forge(client).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            pull_request_id: Some(pull_id(7)),
            commit_sha: Some("bbbbbbb2222222".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();

    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].status, CiJobStatus::Queued);
    assert_eq!(jobs[0].conclusion, None);
    assert_eq!(jobs[0].commit_sha, "bbbbbbb2222222");
}

#[test]
fn combined_pr_and_commit_with_registered_current_run_but_no_tasks_is_empty() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        json!({
            "number": 7,
            "state": "open",
            "user": { "login": "author" },
            "head": { "ref": "feature", "sha": "bbbbbbb2222222" },
            "base": { "ref": "main" },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z"
        })
        .to_string(),
    );
    client.push_response(
        200,
        json!({
            "workflow_runs": [
                {
                    "index_in_repo": 2,
                    "run_number": 2,
                    "status": "queued",
                    "event": "pull_request",
                    "prettyref": "#7",
                    "head_branch": "feature",
                    "head_sha": "bbbbbbb2222222",
                    "created_at": "2024-01-03T00:00:00Z"
                },
                {
                    "index_in_repo": 1,
                    "run_number": 1,
                    "status": "success",
                    "event": "pull_request",
                    "prettyref": "#7",
                    "head_branch": "feature",
                    "head_sha": "aaaaaaa1111111",
                    "created_at": "2024-01-02T00:00:00Z"
                }
            ]
        })
        .to_string(),
    );
    // The old run has a terminal task, but the registered current run has none.
    client.push_response(
        200,
        json!({
            "workflow_runs": [
                task_with_head(1, 1, "build", "success", "aaaaaaa1111111")
            ]
        })
        .to_string(),
    );

    let jobs = block_on(forge(client).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            pull_request_id: Some(pull_id(7)),
            commit_sha: Some("bbbbbbb2222222".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();

    assert!(jobs.is_empty(), "a taskless current run remains pending");
}

#[test]
fn list_by_commit_filters_status_and_sorts_by_name() {
    let client = MockHttpClient::new();
    // No PR detail call: the query targets a commit only.
    client.push_response(
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
        })
        .to_string(),
    );
    client.push_response(
        200,
        json!({
            "workflow_runs": [
                task(1, 20, "zebra", "success"),
                task(2, 20, "alpha", "failure"),
                task(3, 20, "mid", "running")
            ]
        })
        .to_string(),
    );

    let forge = forge(client);
    let query = CiJobQuery {
        commit_sha: Some("abcdef1234567".to_string()),
        status: Some(CiJobStatus::Completed),
        sort: Some(CiJobSort {
            field: CiJobSortField::Name,
            direction: SortDirection::Asc,
        }),
        ..Default::default()
    };
    let jobs = block_on(forge.list_ci_jobs(&repo_id(), query)).unwrap();

    // The running task is filtered out; the rest are sorted by name.
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].name, "alpha");
    assert_eq!(jobs[0].conclusion, Some(CiJobConclusion::Failure));
    assert_eq!(jobs[1].name, "zebra");
    assert_eq!(jobs[1].conclusion, Some(CiJobConclusion::Success));
    assert_eq!(jobs[0].commit_sha, "abcdef1234567");
    // No pull request could be derived from a push run.
    assert_eq!(jobs[0].pull_request_id, None);
}

#[test]
fn list_sort_by_name_descending() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        json!({
            "workflow_runs": [
                {
                    "index_in_repo": 21,
                    "run_number": 21,
                    "status": "success",
                    "event": "push",
                    "head_branch": "main",
                    "head_sha": "abcdef1234567",
                    "created_at": "2024-02-01T00:00:00Z"
                }
            ]
        })
        .to_string(),
    );
    client.push_response(
        200,
        json!({
            "workflow_runs": [
                task(1, 21, "alpha", "success"),
                task(2, 21, "zebra", "success")
            ]
        })
        .to_string(),
    );

    let forge = forge(client);
    let query = CiJobQuery {
        commit_sha: Some("abcdef1234567".to_string()),
        sort: Some(CiJobSort {
            field: CiJobSortField::Name,
            direction: SortDirection::Desc,
        }),
        ..Default::default()
    };
    let jobs = block_on(forge.list_ci_jobs(&repo_id(), query)).unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].name, "zebra");
    assert_eq!(jobs[1].name, "alpha");
}

#[test]
fn list_with_empty_query_returns_all_runs() {
    let client = MockHttpClient::new();
    client.push_response(
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
        })
        .to_string(),
    );
    client.push_response(
        200,
        json!({ "workflow_runs": [ task(1, 30, "build", "success") ] }).to_string(),
    );

    let forge = forge(client);
    let jobs = block_on(forge.list_ci_jobs(&repo_id(), CiJobQuery::default())).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id.as_str(), "forgejo:acme/widgets:actions:30:0:1");
}

#[test]
fn get_ci_job_parses_id_and_returns_job() {
    let client = MockHttpClient::new();
    client.push_response(
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
        })
        .to_string(),
    );
    client.push_response(
        200,
        json!({ "workflow_runs": [ task(1, 30, "build", "success") ] }).to_string(),
    );

    let forge = forge(client);
    let id = CiJobId::new("forgejo:acme/widgets:actions:30:0:1");
    let job = block_on(forge.get_ci_job(&id)).unwrap().unwrap();
    assert_eq!(job.id, id);
    assert_eq!(job.name, "build");
    assert_eq!(job.repo_id, repo_id());
    assert_eq!(job.status, CiJobStatus::Completed);
    assert_eq!(job.conclusion, Some(CiJobConclusion::Success));
}

#[test]
fn get_ci_job_returns_none_for_unknown_run() {
    let client = MockHttpClient::new();
    client.push_response(200, json!({ "workflow_runs": [] }).to_string());

    let forge = forge(client);
    let id = CiJobId::new("forgejo:acme/widgets:actions:99:0:1");
    assert!(block_on(forge.get_ci_job(&id)).unwrap().is_none());
}

#[test]
fn actions_unavailable_is_an_error() {
    let client = MockHttpClient::new();
    // The runs endpoint is unavailable: CI must surface an error, never "passed".
    client.push_response(404, json!({ "message": "Not Found" }).to_string());

    let forge = forge(client);
    let result = block_on(forge.list_ci_jobs(&repo_id(), CiJobQuery::default()));
    assert!(result.is_err());
}
