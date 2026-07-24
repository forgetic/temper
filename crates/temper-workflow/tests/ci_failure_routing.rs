use chrono::{DateTime, Utc};
use temper_forge::{
    BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, ItemNumber, PullRequest,
    PullRequestState,
};
use temper_workflow::{CiState, CiStatus, Classifier, GateSignals, QueueId, RawWorkflowSpec};

fn timestamp() -> DateTime<Utc> {
    "2026-05-29T00:00:00Z".parse().expect("timestamp")
}

fn ci_job(name: &str, conclusion: CiJobConclusion) -> CiJob {
    CiJob {
        id: CiJobId::new(format!("ci-{name}")),
        repo_id: "repo-1".into(),
        pull_request_id: None,
        commit_sha: "head".into(),
        name: name.into(),
        status: CiJobStatus::Completed,
        conclusion: Some(conclusion),
        provider_conclusion: None,
        provider_reason: None,
        run_id: None,
        attempt: None,
        url: None,
        created_at: timestamp(),
        started_at: None,
        completed_at: Some(timestamp()),
        updated_at: timestamp(),
    }
}

#[test]
fn ci_status_retains_structured_latest_terminal_evidence_and_failure_precedence() {
    let mut interrupted = ci_job("validate", CiJobConclusion::RunnerLost);
    interrupted.provider_conclusion = Some("failure".to_string());
    interrupted.provider_reason = Some("runner disconnected".to_string());
    interrupted.run_id = Some("591".to_string());
    interrupted.attempt = Some("2".to_string());
    interrupted.url = Some("https://forge.example/actions/runs/591".to_string());
    let recovery = CiStatus::from_jobs(&[interrupted.clone()]);
    assert!(recovery.is_recovery_required());
    let evidence = &recovery.terminal_evidence()[0];
    assert_eq!(evidence.conclusion, CiJobConclusion::RunnerLost);
    assert_eq!(
        evidence.provider_reason.as_deref(),
        Some("runner disconnected")
    );
    assert_eq!(evidence.run_id.as_deref(), Some("591"));
    assert_eq!(evidence.attempt.as_deref(), Some("2"));

    let ordinary = ci_job("test", CiJobConclusion::Failure);
    let mixed = CiStatus::from_jobs(&[interrupted, ordinary]);
    assert_eq!(mixed.state(), CiState::Failed);
    assert_eq!(mixed.terminal_evidence().len(), 2);
}

#[test]
fn recovery_required_condition_is_distinct_from_ordinary_failure() {
    let workflow = serde_json::from_str::<RawWorkflowSpec>(
        r#"{
            "name": "ci-routing",
            "roles": [{"id": "watcher", "queues": ["failed", "recover"]}],
            "labels": [{"id": "implementation"}],
            "artifact_kinds": [
                {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
            ],
            "queues": [
                {"id": "failed", "artifact": "implementation_pr", "condition": {"kind": "ci_failed"}},
                {"id": "recover", "artifact": "implementation_pr", "condition": {"kind": "ci_recovery_required"}}
            ]
        }"#,
    )
    .expect("workflow parses")
    .validate()
    .expect("workflow validates");
    let artifact = Classifier::new(&workflow)
        .classify_pull_request(&PullRequest {
            id: "pr-1".into(),
            repo_id: "repo-1".into(),
            number: ItemNumber::new(10),
            title: "CI routing".into(),
            body: String::new(),
            state: PullRequestState::Open,
            author_id: "user-1".into(),
            source: BranchRef {
                repository_id: "repo-1".into(),
                branch: "feature".into(),
            },
            target: BranchRef {
                repository_id: "repo-1".into(),
                branch: "main".into(),
            },
            head_sha: Some("head".into()),
            base_sha: None,
            labels: vec!["implementation".into()],
            assignees: Vec::new(),
            requested_reviewers: Vec::new(),
            dependencies: Vec::new(),
            merge: None,
            version: Default::default(),
            created_at: timestamp(),
            updated_at: timestamp(),
            closed_at: None,
        })
        .expect("pull request classifies");
    let planner = workflow.planner();

    let failed =
        planner.matching_queues_with(&artifact, &GateSignals::new().with_ci(CiStatus::failed()));
    assert!(failed.contains(&QueueId::new("failed")));
    assert!(!failed.contains(&QueueId::new("recover")));

    let recovery = planner.matching_queues_with(
        &artifact,
        &GateSignals::new().with_ci(CiStatus::recovery_required()),
    );
    assert!(!recovery.contains(&QueueId::new("failed")));
    assert!(recovery.contains(&QueueId::new("recover")));
}
