// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;
use temper_workflow::{Diagnostic, ReferenceSite, SymbolKind};

fn landing_workflow_with_effect_kind(
    effect_kind: &str,
) -> Result<ValidatedWorkflow, temper_workflow::ValidationErrors> {
    let spec_json = format!(
        r#"{{
  "name": "plan-landing",
  "roles": [{{"id": "tester"}}],
  "labels": [
    {{"id": "plan"}},
    {{"id": "ready"}},
    {{"id": "landing-opened"}},
    {{"id": "feature-landing"}},
    {{"id": "needs-validation"}}
  ],
  "artifact_kinds": [
    {{"id": "plan", "target": "issue", "identifying_labels": ["plan"], "initial_labels": ["ready"]}},
    {{"id": "feature_landing_pr", "target": "pull_request", "identifying_labels": ["feature-landing"], "initial_labels": ["needs-validation"]}}
  ],
  "transitions": [
    {{
      "id": "validate_plan",
      "artifact": "plan",
      "roles": ["tester"],
      "outcomes": {{"passed": "open_feature_landing_pr"}},
      "effects": []
    }},
    {{
      "id": "open_feature_landing_pr",
      "artifact": "plan",
      "roles": ["tester"],
      "effects": [
        {{"kind": "create_pull_request", "artifact_kind": "{effect_kind}"}},
        {{"kind": "remove_label", "label": "ready"}},
        {{"kind": "add_label", "label": "landing-opened"}}
      ]
    }}
  ]
}}"#
    );
    let spec: RawWorkflowSpec = serde_json::from_str(&spec_json).expect("landing workflow json");
    spec.validate()
}

fn landing_workflow() -> ValidatedWorkflow {
    landing_workflow_with_effect_kind("feature_landing_pr").expect("landing workflow validates")
}

async fn create_plan_issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
    target_branch: Option<&str>,
) -> ItemNumber {
    create_plan_issue_with_parents(forge, repo, target_branch, Vec::new()).await
}

async fn create_plan_issue_with_parents(
    forge: &MemoryForge,
    repo: &RepositoryId,
    target_branch: Option<&str>,
    parents: Vec<ArtifactRef>,
) -> ItemNumber {
    let metadata = WorkflowMetadata {
        parents,
        target_branch: target_branch.map(str::to_string),
        ..WorkflowMetadata::default()
    };
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "Validate feature plan".to_string(),
                body: format!(
                    "Plan validation target.\n\n{}",
                    render_metadata_block(&metadata)
                ),
                labels: vec!["plan".to_string(), "ready".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("plan issue is created")
        .number
}

async fn create_feature_issue(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "Product feature".to_string(),
                body: "Build the product feature.".to_string(),
                labels: vec!["feature".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("feature issue is created")
        .number
}

fn plan_validation_job(repo_path: &str, number: ItemNumber) -> InFlightJob {
    job_for_context(
        repo_path,
        number,
        "issue",
        JobContext {
            role: "tester".to_string(),
            repo: repo_path.to_string(),
            queue: "plan_validation".to_string(),
            artifact_kind: "plan".to_string(),
            artifact: None,
            workspace: None,
            action: Some("validate_plan".to_string()),
            checkout_capability: Some("read_only".to_string()),
            allowed_verdicts: vec!["passed".to_string()],
            guidance: None,
            pull_request_freshness: None,
        },
    )
}

#[test]
fn verdict_transition_creates_feature_landing_pr_from_plan_metadata() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_plan_issue(&forge, &repo, Some("feature/144-plan-branch")).await;
        let workflow = Arc::new(landing_workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = plan_validation_job("acme/service", issue);
        let mut result = verdict_result(
            "worker-a",
            &job.job_id,
            "passed",
            Some("# Validation report\n\nFeature branch validation passed."),
        );
        result.title = Some("Land validated feature branch".to_string());

        applier.apply(job, result).await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let pull = &pulls[0];
        assert_eq!(pull.title, "Land validated feature branch");
        assert!(pull.body.starts_with("# Validation report"));
        assert_eq!(pull.source.branch, "feature/144-plan-branch");
        assert_eq!(pull.target.branch, "stable");
        assert_eq!(
            pull.labels,
            vec![
                "feature-landing".to_string(),
                "needs-validation".to_string()
            ]
        );
        let metadata = parse_metadata_block(&pull.body)
            .expect("landing PR metadata parses")
            .expect("landing PR metadata exists");
        assert_eq!(
            metadata.kind,
            Some(ArtifactKindId::new("feature_landing_pr"))
        );
        assert_eq!(metadata.parents, vec![ArtifactRef::same_repo(issue)]);
        assert_eq!(
            metadata.correlation_key.as_deref(),
            Some(format!("pr-for-plan-{}", issue.get()).as_str())
        );

        let labels = issue_labels(&forge, &repo, issue).await;
        assert!(has_label(&labels, "plan"), "plan label remains: {labels:?}");
        assert!(
            has_label(&labels, "landing-opened"),
            "landing-opened label is applied: {labels:?}"
        );
        assert!(
            !has_label(&labels, "ready"),
            "ready label is cleared: {labels:?}"
        );
    })
}

#[test]
fn verdict_transition_carries_source_parents_into_feature_landing_pr_metadata() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let feature = create_feature_issue(&forge, &repo).await;
        let issue = create_plan_issue_with_parents(
            &forge,
            &repo,
            Some("feature/144-plan-branch"),
            vec![ArtifactRef::same_repo(feature)],
        )
        .await;
        let workflow = Arc::new(landing_workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = plan_validation_job("acme/service", issue);
        let result = verdict_result("worker-a", &job.job_id, "passed", None);

        applier.apply(job, result).await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let metadata = parse_metadata_block(&pulls[0].body)
            .expect("landing PR metadata parses")
            .expect("landing PR metadata exists");
        assert_eq!(
            metadata.parents,
            vec![
                ArtifactRef::same_repo(issue),
                ArtifactRef::same_repo(feature)
            ]
        );
    })
}

#[test]
fn verdict_transition_deduplicates_source_parent_refs_in_landing_pr_metadata() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let feature = create_feature_issue(&forge, &repo).await;
        let issue = create_plan_issue_with_parents(
            &forge,
            &repo,
            Some("feature/144-plan-branch"),
            vec![
                ArtifactRef::same_repo(feature),
                ArtifactRef::same_repo(feature),
            ],
        )
        .await;
        let workflow = Arc::new(landing_workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = plan_validation_job("acme/service", issue);
        let result = verdict_result("worker-a", &job.job_id, "passed", None);

        applier.apply(job, result).await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let metadata = parse_metadata_block(&pulls[0].body)
            .expect("landing PR metadata parses")
            .expect("landing PR metadata exists");
        assert_eq!(
            metadata.parents,
            vec![
                ArtifactRef::same_repo(issue),
                ArtifactRef::same_repo(feature)
            ]
        );
    })
}

#[test]
fn verdict_feature_landing_pr_replay_is_idempotent() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_plan_issue(&forge, &repo, Some("feature/144-plan-branch")).await;
        let workflow = Arc::new(landing_workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = plan_validation_job("acme/service", issue);
        let result = verdict_result("worker-a", &job.job_id, "passed", None);

        applier.apply(job.clone(), result.clone()).await;
        wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        applier.apply(job, result).await;

        assert_pull_request_count_stays(&cx, &forge, &repo, 1).await;
    })
}

#[test]
fn verdict_feature_landing_pr_requires_plan_target_branch_before_mutation() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_plan_issue(&forge, &repo, None).await;
        let workflow = Arc::new(landing_workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = plan_validation_job("acme/service", issue);
        let result = verdict_result("worker-a", &job.job_id, "passed", None);

        applier.apply(job, result).await;

        assert_pull_request_count_stays(&cx, &forge, &repo, 0).await;
        let labels = issue_labels(&forge, &repo, issue).await;
        assert_eq!(labels, vec!["plan".to_string(), "ready".to_string()]);
    })
}

#[test]
fn create_pull_request_artifact_kind_must_name_pr_kind() {
    let unknown = landing_workflow_with_effect_kind("missing_landing_pr")
        .expect_err("unknown landing PR kind is rejected");
    assert!(unknown.diagnostics().iter().any(|diagnostic| {
        matches!(
            diagnostic,
            Diagnostic::UndeclaredReference {
                expected: SymbolKind::ArtifactKind,
                id,
                site: ReferenceSite::TransitionEffectArtifactKind { transition },
            } if id == "missing_landing_pr" && transition == "open_feature_landing_pr"
        )
    }));

    let non_pr = landing_workflow_with_effect_kind("plan")
        .expect_err("non-PR landing artifact kind is rejected");
    assert!(non_pr.diagnostics().iter().any(|diagnostic| {
        matches!(
            diagnostic,
            Diagnostic::CreatePullRequestArtifactKindTargetMismatch {
                transition,
                artifact_kind,
                target,
            } if transition == "open_feature_landing_pr"
                && artifact_kind == "plan"
                && target == "issue"
        )
    }));
}
