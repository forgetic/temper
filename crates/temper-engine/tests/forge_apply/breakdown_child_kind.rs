// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

const PLAN_CENTRIC_WORKFLOW: &str =
    include_str!("../../../../scenarios/plan-centric-feature-branch/config/workflow.json");

fn plan_centric_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(PLAN_CENTRIC_WORKFLOW).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn body_with_target_branch(body: &str, target_branch: &str) -> String {
    format!(
        "{body}\n\n{}",
        render_metadata_block(&WorkflowMetadata {
            target_branch: Some(target_branch.to_string()),
            ..WorkflowMetadata::default()
        })
    )
}

fn plan_feature_in_flight_job(repo_path: &str, number: ItemNumber) -> InFlightJob {
    job_for_context(
        repo_path,
        number,
        "issue",
        JobContext {
            role: "architect".to_string(),
            repo: repo_path.to_string(),
            queue: "feature_planning".to_string(),
            artifact_kind: "feature".to_string(),
            artifact: None,
            workspace: None,
            action: Some("plan_feature".to_string()),
            checkout_capability: Some("read_only".to_string()),
            allowed_verdicts: vec!["needs_plan".to_string(), "config_only".to_string()],
            guidance: None,
            pull_request_freshness: None,
        },
    )
}

async fn create_feature_issue(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "feature".to_string(),
                body: "build the feature".to_string(),
                labels: vec!["feature".to_string()],
                assignees: Vec::<UserId>::new(),
            },
        )
        .await
        .expect("feature issue is created")
        .number
}

async fn set_issue_body(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    body: String,
) {
    let issue = forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists");
    forge
        .update_issue(
            &issue.id,
            UpdateIssue {
                body: Some(body),
                expected_version: Some(issue.version),
                ..UpdateIssue::default()
            },
        )
        .await
        .expect("issue body is updated");
}

#[test]
fn plan_centric_workflow_rejects_feature_direct_code_child() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let issue = create_feature_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));
        let job = plan_feature_in_flight_job("acme/service", issue);
        let before = issue_body_and_labels(&forge, &repo, issue).await;
        let mut direct_code = job_child(
            "direct-code",
            "Implement directly from feature",
            "This must be rejected because code children belong under a plan.",
            &[],
        );
        direct_code.kind = Some("code".to_string());
        let result =
            verdict_result_with_children("worker-a", &job.job_id, "needs_plan", vec![direct_code]);

        applier.apply(job, result).await;

        assert_eq!(issue_body_and_labels(&forge, &repo, issue).await, before);
        assert_eq!(list_issues(&forge, &repo).await.len(), 1);
    })
}

#[test]
fn omitted_child_kind_defaults_to_ready_code_child() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);
        let result = verdict_result_with_children(
            "worker-a",
            &job.job_id,
            "needs_breakdown",
            vec![job_child(
                "api-schema",
                "Define the API schema",
                "Write the shared API schema.",
                &[],
            )],
        );

        applier.apply(job, result).await;

        let issues = list_issues(&forge, &repo).await;
        assert_eq!(issues.len(), 2);
        let child = issue_by_slug(&issues, "api-schema");
        assert_eq!(child.labels, vec!["code".to_string(), "ready".to_string()]);
        let metadata = parse_metadata_block(&child.body)
            .expect("child metadata parses")
            .expect("child metadata exists");
        assert_eq!(metadata.kind, Some(ArtifactKindId::new("code")));
        assert!(metadata.target_branch.is_none());
    })
}

#[test]
fn source_target_branch_is_inherited_by_code_child_without_child_target() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        set_issue_body(
            &forge,
            &repo,
            issue,
            body_with_target_branch("rough user request", "feature/plan-work"),
        )
        .await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);
        let result = verdict_result_with_children(
            "worker-a",
            &job.job_id,
            "needs_breakdown",
            vec![job_child(
                "api-schema",
                "Define the API schema",
                "Write the shared API schema.",
                &[],
            )],
        );

        applier.apply(job, result).await;

        let issues = list_issues(&forge, &repo).await;
        assert_eq!(issues.len(), 2);
        let child = issue_by_slug(&issues, "api-schema");
        let metadata = parse_metadata_block(&child.body)
            .expect("child metadata parses")
            .expect("child metadata exists");
        assert_eq!(metadata.kind, Some(ArtifactKindId::new("code")));
        assert_eq!(metadata.parents, vec![ArtifactRef::same_repo(issue)]);
        assert_eq!(metadata.target_branch.as_deref(), Some("feature/plan-work"));
    })
}

#[test]
fn explicit_plan_child_uses_plan_labels_and_metadata() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow_with_plan_kind()));
        let job = triage_in_flight_job("acme/service", issue);
        let mut child = job_child(
            "implementation-plan",
            "Plan the implementation",
            "Plan the implementation work.",
            &[],
        );
        child.kind = Some("plan".to_string());
        let result =
            verdict_result_with_children("worker-a", &job.job_id, "needs_breakdown", vec![child]);

        applier.apply(job, result).await;

        let issues = list_issues(&forge, &repo).await;
        assert_eq!(issues.len(), 2);
        let child = issue_by_slug(&issues, "implementation-plan");
        assert_eq!(child.labels, vec!["plan".to_string()]);
        assert!(!has_label(&child.labels, "code"));
        assert!(!has_label(&child.labels, "ready"));
        let metadata = parse_metadata_block(&child.body)
            .expect("child metadata parses")
            .expect("child metadata exists");
        assert_eq!(metadata.kind, Some(ArtifactKindId::new("plan")));
    })
}

#[test]
fn blocked_code_child_does_not_receive_ready_initial_label() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);
        let mut blocked = job_child(
            "web-client",
            "Implement the web client",
            "Build the web client after the API schema lands.",
            &["blocked"],
        );
        blocked.kind = Some("code".to_string());
        blocked.depends_on = vec!["api-schema".to_string()];
        let result = verdict_result_with_children(
            "worker-a",
            &job.job_id,
            "needs_breakdown",
            vec![
                job_child(
                    "api-schema",
                    "Define the API schema",
                    "Write the shared API schema.",
                    &[],
                ),
                blocked,
            ],
        );

        applier.apply(job, result).await;

        let issues = list_issues(&forge, &repo).await;
        assert_eq!(issues.len(), 3);
        let blocked = issue_by_slug(&issues, "web-client");
        assert_eq!(blocked.labels.len(), 2);
        assert!(has_label(&blocked.labels, "code"));
        assert!(has_label(&blocked.labels, "blocked"));
        assert!(!has_label(&blocked.labels, "ready"));
        let metadata = parse_metadata_block(&blocked.body)
            .expect("blocked child metadata parses")
            .expect("blocked child metadata exists");
        assert_eq!(metadata.kind, Some(ArtifactKindId::new("code")));
    })
}

#[test]
fn child_metadata_target_branch_is_preserved_while_kind_is_stamped_or_kept() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        set_issue_body(
            &forge,
            &repo,
            issue,
            body_with_target_branch("rough user request", "feature/source-work"),
        )
        .await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow_with_plan_kind()));
        let job = triage_in_flight_job("acme/service", issue);

        let plan_body = format!(
            "Draft the plan.\n\n{}",
            render_metadata_block(&WorkflowMetadata {
                target_branch: Some("feature/plan-work".to_string()),
                ..WorkflowMetadata::default()
            })
        );
        let mut plan = job_child("plan", "Plan the work", &plan_body, &[]);
        plan.kind = Some("plan".to_string());

        let code_body = format!(
            "Implement the work.\n\n{}",
            render_metadata_block(&WorkflowMetadata {
                kind: Some(ArtifactKindId::new("code")),
                target_branch: Some("feature/code-work".to_string()),
                ..WorkflowMetadata::default()
            })
        );
        let mut code = job_child("code-child", "Implement the work", &code_body, &[]);
        code.kind = Some("code".to_string());

        let result = verdict_result_with_children(
            "worker-a",
            &job.job_id,
            "needs_breakdown",
            vec![plan, code],
        );

        applier.apply(job, result).await;

        let issues = list_issues(&forge, &repo).await;
        assert_eq!(issues.len(), 3);
        let plan = issue_by_slug(&issues, "plan");
        let plan_metadata = parse_metadata_block(&plan.body)
            .expect("plan metadata parses")
            .expect("plan metadata exists");
        assert_eq!(plan_metadata.kind, Some(ArtifactKindId::new("plan")));
        assert_eq!(
            plan_metadata.target_branch.as_deref(),
            Some("feature/plan-work")
        );

        let code = issue_by_slug(&issues, "code-child");
        let code_metadata = parse_metadata_block(&code.body)
            .expect("code metadata parses")
            .expect("code metadata exists");
        assert_eq!(code_metadata.kind, Some(ArtifactKindId::new("code")));
        assert_eq!(
            code_metadata.target_branch.as_deref(),
            Some("feature/code-work")
        );
    })
}

#[test]
fn malformed_source_metadata_aborts_without_children_or_label_effects() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        set_issue_body(
            &forge,
            &repo,
            issue,
            "rough user request\n\n<!-- temper:workflow\n{ not valid json }\n-->".to_string(),
        )
        .await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);
        let before = issue_body_and_labels(&forge, &repo, issue).await;
        let result = verdict_result_with_children(
            "worker-a",
            &job.job_id,
            "needs_breakdown",
            vec![job_child(
                "api-schema",
                "Define the API schema",
                "Write the shared API schema.",
                &[],
            )],
        );

        applier.apply(job, result).await;

        assert_eq!(issue_body_and_labels(&forge, &repo, issue).await, before);
        assert_eq!(list_issues(&forge, &repo).await.len(), 1);
    })
}

#[test]
fn cross_repo_children_inherit_source_target_branch_unless_overridden() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo_a = new_repo(&forge, "stable").await;
        let repo_b = create_repo(&forge, "acme", "web", "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo_a).await;
        set_issue_body(
            &forge,
            &repo_a,
            issue,
            body_with_target_branch("rough user request", "feature/source-plan"),
        )
        .await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);

        let mut inherited = job_child(
            "web-client",
            "Implement the web client",
            "Build the web client against the API schema.",
            &[],
        );
        inherited.target_repo = Some("acme/web".to_string());

        let override_body = body_with_target_branch("Build the admin UI.", "feature/admin-ui");
        let mut overridden = job_child("admin-ui", "Implement the admin UI", &override_body, &[]);
        overridden.target_repo = Some("acme/web".to_string());

        let result = verdict_result_with_children(
            "worker-a",
            &job.job_id,
            "needs_breakdown",
            vec![inherited, overridden],
        );

        applier.apply(job, result).await;

        assert_eq!(list_issues(&forge, &repo_a).await.len(), 1);
        let web_issues = list_issues(&forge, &repo_b).await;
        assert_eq!(web_issues.len(), 2);
        let inherited = issue_by_slug(&web_issues, "web-client");
        let inherited_metadata = parse_metadata_block(&inherited.body)
            .expect("inherited child metadata parses")
            .expect("inherited child metadata exists");
        assert_eq!(
            inherited_metadata.target_branch.as_deref(),
            Some("feature/source-plan")
        );
        assert_eq!(inherited_metadata.kind, Some(ArtifactKindId::new("code")));
        assert_eq!(
            inherited_metadata.parents,
            vec![ArtifactRef::in_repo(repo_a.clone(), issue)]
        );

        let overridden = issue_by_slug(&web_issues, "admin-ui");
        let overridden_metadata = parse_metadata_block(&overridden.body)
            .expect("overridden child metadata parses")
            .expect("overridden child metadata exists");
        assert_eq!(
            overridden_metadata.target_branch.as_deref(),
            Some("feature/admin-ui")
        );
        assert_eq!(overridden_metadata.kind, Some(ArtifactKindId::new("code")));
    })
}

#[test]
fn unknown_or_malformed_child_kind_drops_verdict_apply_without_partial_children() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);
        let before = issue_body_and_labels(&forge, &repo, issue).await;
        let mut unknown = job_child(
            "unknown-kind",
            "Unknown child kind",
            "This child must make the whole apply fail.",
            &[],
        );
        unknown.kind = Some("not-a-workflow-kind".to_string());
        let result = verdict_result_with_children(
            "worker-a",
            &job.job_id,
            "needs_breakdown",
            vec![
                job_child(
                    "api-schema",
                    "Define the API schema",
                    "Write the shared API schema.",
                    &[],
                ),
                unknown,
            ],
        );

        applier.apply(job.clone(), result).await;

        assert_eq!(issue_body_and_labels(&forge, &repo, issue).await, before);
        assert_eq!(list_issues(&forge, &repo).await.len(), 1);

        let mut malformed = job_child(
            "empty-kind",
            "Empty child kind",
            "An empty kind is malformed and must not create any siblings.",
            &[],
        );
        malformed.kind = Some("  ".to_string());
        let result = verdict_result_with_children(
            "worker-a",
            &job.job_id,
            "needs_breakdown",
            vec![
                job_child(
                    "api-schema",
                    "Define the API schema",
                    "Write the shared API schema.",
                    &[],
                ),
                malformed,
            ],
        );

        applier.apply(job, result).await;

        assert_eq!(issue_body_and_labels(&forge, &repo, issue).await, before);
        assert_eq!(list_issues(&forge, &repo).await.len(), 1);
    })
}
