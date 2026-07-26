//! Regression for conservative Forgejo status-only failure evidence.

mod support;

use serde_json::json;
use support::{MockHttpClient, block_on, forge, repo_id};
use temper_forge_model::{CiJobConclusion, CiJobQuery, CiJobStatus};

#[test]
fn run_591_status_only_failure_remains_ambiguous_and_recovery_required() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        json!({
            "workflow_runs": [{
                "id": 591,
                "status": "failure",
                "event": "push",
                "head_sha": "c456eec18b00",
                "created_at": "2026-07-23T15:23:00Z",
                "updated_at": "2026-07-23T15:38:32Z"
            }]
        })
        .to_string(),
    );
    client.push_response(200, include_str!("fixtures/run-591-jobs.json"));

    let jobs = block_on(forge(client.clone()).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some("c456eec18b00".to_string()),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(
        job.id.as_str(),
        "forgejo:acme/widgets:actions:591:3385:1:3385"
    );
    assert_eq!(job.status, CiJobStatus::Completed);
    assert_eq!(job.conclusion, Some(CiJobConclusion::Unknown));
    assert_eq!(job.provider_conclusion.as_deref(), Some("failure"));
    assert_eq!(job.provider_reason, None);
    assert_eq!(job.run_id.as_deref(), Some("591"));
    assert_eq!(job.attempt.as_deref(), Some("1"));
    assert!(temper_workflow::CiStatus::from_jobs(&jobs).is_recovery_required());

    let recorded = client.recorded();
    assert_eq!(
        recorded[1].path,
        "/api/v1/repos/acme/widgets/actions/runs/591/jobs"
    );
    assert!(recorded.iter().all(|request| {
        !request.path.contains("/actions/tasks")
            && !request.path.contains("/user/login")
            && request.path.starts_with("/api/v1/")
    }));
}
