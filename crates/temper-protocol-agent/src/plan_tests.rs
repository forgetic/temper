use super::*;

#[test]
fn step_progress_accepts_legacy_json_without_plan_publication() {
    let parsed: StepProgress = serde_json::from_str(
        r#"{"correlation_key":"k","step":1,"status":"start","state":"started"}"#,
    )
    .expect("legacy progress parses");
    assert_eq!(parsed.plan_publication, None);
}

#[test]
fn step_progress_round_trips_plan_publication() {
    let progress = StepProgress {
        correlation_key: "pr-for-code-7".to_string(),
        step: 1,
        status: "publish implementation plan".to_string(),
        state: StepState::Done,
        pushed_sha: None,
        note: Some("ready to implement".to_string()),
        plan_publication: Some(PlanPublication {
            summary: "Implement deterministic notes".to_string(),
            phases: vec!["create notes file".to_string(), "verify result".to_string()],
            target_repos: vec![PlanPublicationTarget {
                repo_path: "acme/demo".to_string(),
                dir: "demo".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("agent/pr-for-code-7".to_string()),
            }],
        }),
    };

    let line = progress.to_line().expect("serialize");
    assert!(line.contains("plan_publication"));
    let parsed = StepProgress::from_line(&line)
        .expect("parse")
        .expect("non-empty");
    assert_eq!(parsed, progress);
}

#[test]
fn plan_publication_fills_targets_from_workspace_context() {
    let context = WorkspaceContext {
        repos: vec![
            WorkspaceRepository {
                id: "repo-1".to_string(),
                owner: "acme".to_string(),
                name: "demo".to_string(),
                default_branch: "main".to_string(),
                dir: "demo".to_string(),
                access: "writable".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("agent/pr-for-code-7".to_string()),
            },
            WorkspaceRepository {
                id: "repo-2".to_string(),
                owner: "acme".to_string(),
                name: "support".to_string(),
                default_branch: "main".to_string(),
                dir: "support".to_string(),
                access: "read_only".to_string(),
                base_branch: "main".to_string(),
                branch_hint: None,
            },
        ],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: 7 }".to_string(),
            context: "{}".to_string(),
        },
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-7".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: Vec::new(),
        guidance: WorkspaceGuidance::default(),
    };

    let publication = PlanPublication::from_context(
        "Implement deterministic notes",
        vec!["create notes file".to_string(), "verify result".to_string()],
        &context,
    );
    assert_eq!(publication.summary, "Implement deterministic notes");
    assert_eq!(publication.phases.len(), 2);
    assert_eq!(publication.target_repos.len(), 1);
    let target = &publication.target_repos[0];
    assert_eq!(target.repo_path, "acme/demo");
    assert_eq!(target.dir, "demo");
    assert_eq!(target.base_branch, "main");
    assert_eq!(target.branch_hint.as_deref(), Some("agent/pr-for-code-7"));
}
