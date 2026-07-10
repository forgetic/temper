use super::*;

#[test]
fn enrich_work_item_job_globally_skips_attention_artifact() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id;
        let issue = forge
            .create_issue(
                &repo,
                temper_forge::CreateIssue {
                    title: "parked".to_string(),
                    body: "requires operator attention".to_string(),
                    labels: vec![
                        "code".to_string(),
                        "ready".to_string(),
                        "needs-human".to_string(),
                    ],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("issue is created");
        let item = work_item(ArtifactSource::Issue {
            number: issue.number,
        });
        let mut job = job_from_work_item("ai/temper", &item);
        let spec: RawWorkflowSpec =
            serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("workflow parses");
        let workflow = spec.validate().expect("workflow validates");
        let compiled = workflow.compile();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("attention artifact is skipped"),
            EnrichOutcome::SkipAttentionArtifact
        );
    })
}
