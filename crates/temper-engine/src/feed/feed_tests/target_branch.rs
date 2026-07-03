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
