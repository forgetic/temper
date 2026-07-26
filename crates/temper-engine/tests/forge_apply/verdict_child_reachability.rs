// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::breakdown_child_kind::plan_centric_workflow;
use crate::support::*;

async fn create_ready_plan(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "Plan current-head CI landing".to_string(),
                body: format!(
                    "Plan the feature.\n\n{}",
                    render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("plan")),
                        target_branch: Some("feature/current-head-ci".to_string()),
                        ..WorkflowMetadata::default()
                    })
                ),
                labels: vec!["plan".to_string(), "ready".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("plan issue is created")
        .number
}

fn decompose_plan_job(repo_path: &str, number: ItemNumber) -> InFlightJob {
    job_for_context(
        repo_path,
        number,
        "issue",
        JobContext {
            trace_context: None,
            artifact_context: None,
            role: "architect".to_string(),
            repo: repo_path.to_string(),
            queue: "plan_ready".to_string(),
            artifact_kind: "plan".to_string(),
            artifact: None,
            workspace: None,
            action: Some("decompose_plan".to_string()),
            checkout_capability: Some("read_only".to_string()),
            allowed_verdicts: vec!["children_ready".to_string(), "config_only".to_string()],
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: None,
            structured_guidance: None,
            pull_request_freshness: None,
        },
    )
}

fn product(slug: &str) -> JobChild {
    let mut child = job_child(
        slug,
        &format!("Implement {slug}"),
        "Implement product work.",
        &[],
    );
    child.kind = Some("code".to_string());
    child
}

fn scenario(dependencies: &[&str]) -> JobChild {
    let mut child = job_child(
        "feature-scenario",
        "Author the feature scenario",
        "Author the required checked-in feature scenario.",
        &[],
    );
    child.kind = Some("validation".to_string());
    child.depends_on = dependencies
        .iter()
        .map(|slug| (*slug).to_string())
        .collect();
    child
}

async fn rejected_decomposition(children: Vec<JobChild>, expected: &str) {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "main").await;
    let plan = create_ready_plan(&forge, &repo).await;
    let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));
    let job = decompose_plan_job("acme/service", plan);
    let result = verdict_result_with_children("worker-a", &job.job_id, "children_ready", children);

    applier.apply(job, result).await;

    let (_, labels) = issue_body_and_labels(&forge, &repo, plan).await;
    assert!(has_label(&labels, "plan"));
    assert!(has_label(&labels, "ready"));
    assert!(has_label(&labels, "needs-human"));
    assert_eq!(list_issues(&forge, &repo).await.len(), 1);
    let comments = issue_comment_bodies(&forge, &repo, plan).await;
    assert_eq!(comments.len(), 1);
    assert!(comments[0].contains(expected), "rejection: {}", comments[0]);
}

#[test]
fn plan_decomposition_requires_exactly_one_validation_child_before_forge_mutation() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        rejected_decomposition(
            vec![product("api")],
            "exactly 1 child product(s) of kind `validation`, received 0",
        )
        .await;
        rejected_decomposition(
            vec![product("api"), scenario(&["api"]), scenario(&["api"])],
            "exactly 1 child product(s) of kind `validation`, received 2",
        )
        .await;
    })
}

#[test]
fn validation_child_must_depend_on_every_product_before_forge_mutation() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        rejected_decomposition(
            vec![product("api"), product("ui"), scenario(&["api"])],
            "must depend on every `code` child; missing: ui",
        )
        .await;
    })
}

#[test]
fn valid_validation_child_inherits_branch_and_starts_blocked() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let plan = create_ready_plan(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));
        let job = decompose_plan_job("acme/service", plan);
        let result = verdict_result_with_children(
            "worker-a",
            &job.job_id,
            "children_ready",
            vec![product("api"), product("ui"), scenario(&["api", "ui"])],
        );

        applier.apply(job, result).await;

        let issues = list_issues(&forge, &repo).await;
        assert_eq!(issues.len(), 4);
        let scenario = issue_by_slug(&issues, "feature-scenario");
        assert!(has_label(&scenario.labels, "validation"));
        assert!(has_label(&scenario.labels, "blocked"));
        assert!(!has_label(&scenario.labels, "ready"));
        let metadata = parse_metadata_block(&scenario.body)
            .expect("scenario metadata parses")
            .expect("scenario metadata exists");
        assert_eq!(metadata.dependencies.len(), 2);
        assert_eq!(metadata.kind, Some(ArtifactKindId::new("validation")));
        assert_eq!(
            metadata.target_branch.as_deref(),
            Some("feature/current-head-ci")
        );
        let (_, plan_labels) = issue_body_and_labels(&forge, &repo, plan).await;
        assert!(has_label(&plan_labels, "in-progress"));
        assert!(!has_label(&plan_labels, "needs-validation"));
    })
}
