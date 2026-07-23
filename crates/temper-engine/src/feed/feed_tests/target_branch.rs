use super::*;

async fn new_repo(forge: &MemoryForge) -> RepositoryId {
    forge
        .create_repository(CreateRepository {
            owner: "ai".to_string(),
            name: "temper".to_string(),
            default_branch: "main".to_string(),
            description: None,
        })
        .await
        .expect("repository is created")
        .id
}

fn basic_workflow() -> (ValidatedWorkflow, CompiledWorkflow) {
    let workflow: RawWorkflowSpec =
        serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
    let workflow = workflow
        .validate()
        .expect("basic-delivery workflow validates");
    let compiled = workflow.compile();
    (workflow, compiled)
}

fn plan_centric_workflow() -> (ValidatedWorkflow, CompiledWorkflow) {
    let workflow: RawWorkflowSpec = serde_json::from_str(include_str!(
        "../../../../../scenarios/plan-centric-feature-branch/config/workflow.json"
    ))
    .expect("plan-centric workflow parses");
    let workflow = workflow
        .validate()
        .expect("plan-centric workflow validates");
    let compiled = workflow.compile();
    (workflow, compiled)
}

async fn create_code_issue(forge: &MemoryForge, repo: &RepositoryId, body: String) -> ItemNumber {
    forge
        .create_issue(
            repo,
            temper_forge::CreateIssue {
                title: "branch target work".to_string(),
                body,
                labels: vec!["code".to_string(), "ready".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("issue is created")
        .number
}

async fn enriched_primary_base_branch(
    forge: &MemoryForge,
    repo: &RepositoryId,
    issue: ItemNumber,
) -> (String, String) {
    let item = work_item(ArtifactSource::Issue { number: issue });
    let mut job = job_from_work_item("ai/temper", &item);
    let (workflow, compiled) = basic_workflow();

    assert_eq!(
        enrich_work_item_job(forge, repo, &item, &mut job, &workflow, &compiled)
            .await
            .expect("enrichment succeeds"),
        EnrichOutcome::Enriched
    );

    let context: JobContext =
        serde_json::from_value(job.job_payload).expect("enriched JobContext parses");
    let primary = context
        .workspace
        .as_ref()
        .expect("enriched job carries a workspace manifest")
        .primary()
        .expect("primary repo present");
    (primary.default_branch.clone(), primary.base_branch.clone())
}

#[test]
fn enrich_issue_workspace_uses_metadata_target_branch_as_base_branch() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_code_issue(
            &forge,
            &repo,
            format!(
                "needs implementation\n\n{}",
                render_metadata_block(&WorkflowMetadata {
                    target_branch: Some("feature/144-plan-branch".to_string()),
                    ..WorkflowMetadata::default()
                })
            ),
        )
        .await;

        let (default_branch, base_branch) =
            enriched_primary_base_branch(&forge, &repo, issue).await;
        assert_eq!(default_branch, "main");
        assert_eq!(base_branch, "feature/144-plan-branch");
    })
}

#[test]
fn enrich_issue_workspace_uses_default_branch_without_target_branch_metadata() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_code_issue(&forge, &repo, "needs implementation".to_string()).await;

        let (default_branch, base_branch) =
            enriched_primary_base_branch(&forge, &repo, issue).await;
        assert_eq!(default_branch, "main");
        assert_eq!(base_branch, "main");
    })
}

#[test]
fn plan_centric_code_assignment_uses_validated_feature_branch_workspace_base() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_code_issue(
            &forge,
            &repo,
            format!(
                "needs implementation\n\n{}",
                render_metadata_block(&WorkflowMetadata {
                    target_branch: Some("agent/pr-for-feature-620".to_string()),
                    ..WorkflowMetadata::default()
                })
            ),
        )
        .await;
        let item = work_item(ArtifactSource::Issue { number: issue });
        let mut job = job_from_work_item("ai/temper", &item);
        let (workflow, compiled) = plan_centric_workflow();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("validated feature branch assignment succeeds"),
            EnrichOutcome::Enriched
        );

        let context: JobContext = serde_json::from_value(job.job_payload).expect("context parses");
        let primary = context
            .workspace
            .expect("workspace exists")
            .repos
            .into_iter()
            .next()
            .expect("primary exists");
        assert_eq!(primary.default_branch, "main");
        assert_eq!(primary.base_branch, "agent/pr-for-feature-620");
    })
}

#[test]
fn plan_centric_code_assignment_rejects_missing_or_default_target_branch() {
    temper_engine_io::block_on(async move {
        for target_branch in [None, Some("main")] {
            let forge = MemoryForge::new();
            let repo = new_repo(&forge).await;
            let body = match target_branch {
                Some(branch) => format!(
                    "needs implementation\n\n{}",
                    render_metadata_block(&WorkflowMetadata {
                        target_branch: Some(branch.to_string()),
                        ..WorkflowMetadata::default()
                    })
                ),
                None => "needs implementation".to_string(),
            };
            let issue = create_code_issue(&forge, &repo, body).await;
            let item = work_item(ArtifactSource::Issue { number: issue });
            let mut job = job_from_work_item("ai/temper", &item);
            let (workflow, compiled) = plan_centric_workflow();

            let error = enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect_err("invalid feature target cannot be assigned");
            let error = error.to_string();
            assert!(
                error.contains("target_branch") || error.contains("non-default target branch"),
                "unexpected error: {error}"
            );
        }
    })
}

#[test]
fn plan_feature_assignment_exposes_engine_resolved_branch_contract() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = forge
            .create_issue(
                &repo,
                temper_forge::CreateIssue {
                    title: "feature".to_string(),
                    body: format!(
                        "Plan this feature.\n\n{}",
                        render_metadata_block(&WorkflowMetadata {
                            target_branch: Some("main".to_string()),
                            ..WorkflowMetadata::default()
                        })
                    ),
                    labels: vec!["feature".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("feature is created");
        let spec: RawWorkflowSpec = serde_json::from_str(include_str!(
            "../../../../../scenarios/plan-centric-feature-branch/config/workflow.json"
        ))
        .expect("plan-centric workflow parses");
        let workflow = spec.validate().expect("plan-centric workflow validates");
        let compiled = workflow.compile();
        let item = WorkItem {
            queue: QueueId::new("feature_planning"),
            role: RoleId::new("architect"),
            target: ArtifactSource::Issue {
                number: issue.number,
            },
            kind: ArtifactKindId::new("feature"),
        };
        let mut job = job_from_work_item("ai/temper", &item);

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment succeeds"),
            EnrichOutcome::Enriched
        );

        let context: JobContext = serde_json::from_value(job.job_payload).expect("context parses");
        let requirement = context.verdict_contracts["needs_plan"]
            .target_branch
            .as_ref()
            .expect("resolved branch requirement");
        assert_eq!(
            requirement.expected,
            format!("agent/pr-for-feature-{}", issue.number.get())
        );
        assert_eq!(requirement.repository_default, "main");
        assert!(requirement.allow_omission);
    })
}
