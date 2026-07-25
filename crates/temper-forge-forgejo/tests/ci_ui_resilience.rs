// SPDX-License-Identifier: MPL-2.0
//! Fail-closed API-only behavior replaces web-UI resilience heuristics.
mod support;

use serde_json::json;
use support::{MockHttpClient, block_on, forge_with_web_ui, repo_id};
use temper_forge_model::{CiJobQuery, ForgeError};

#[test]
fn unreadable_per_run_jobs_never_falls_back_to_live_view() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        json!({
            "workflow_runs": [{
                "id": 900,
                "index_in_repo": 10,
                "head_sha": "abcdef1234567",
                "status": "queued"
            }]
        })
        .to_string(),
    );
    client.push_response(403, json!({ "message": "Forbidden" }).to_string());

    let result = block_on(forge_with_web_ui(client.clone()).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some("abcdef1234567".to_string()),
            ..Default::default()
        },
    ));
    assert!(matches!(result, Err(ForgeError::Backend(_))));

    let recorded = client.recorded();
    assert_eq!(recorded.len(), 2);
    assert_eq!(
        recorded[1].path,
        "/api/v1/repos/acme/widgets/actions/runs/900/jobs"
    );
    assert!(recorded.iter().all(|request| {
        request.path.starts_with("/api/v1/")
            && !request.path.contains("/actions/tasks")
            && !request.path.contains("/user/login")
    }));
}
