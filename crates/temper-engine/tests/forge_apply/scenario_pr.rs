// SPDX-License-Identifier: MPL-2.0

use crate::support::*;

#[test]
fn scenario_author_success_creates_distinct_pr_into_feature_branch() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let issue = forge
            .create_issue(
                &repo,
                CreateIssue {
                    title: "Author the feature scenario".to_string(),
                    body: format!(
                        "Author scenario.\n\n{}",
                        render_metadata_block(&WorkflowMetadata {
                            kind: Some(ArtifactKindId::new("validation")),
                            target_branch: Some("feature/scenario-lifecycle".to_string()),
                            ..WorkflowMetadata::default()
                        })
                    ),
                    labels: vec!["validation".to_string(), "ready".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("validation issue is created")
            .number;
        let mut manifest_repo = writable_repo("acme/service", "agent/scenario-author");
        manifest_repo.base_branch = "feature/scenario-lifecycle".to_string();
        let job = job_for_context(
            "acme/service",
            issue,
            "issue",
            JobContext {
                trace_context: None,
                artifact_context: None,
                role: "scenario_author".to_string(),
                repo: "acme/service".to_string(),
                queue: "validation_ready".to_string(),
                artifact_kind: "validation".to_string(),
                artifact: None,
                workspace: Some(WorkspaceManifest {
                    coordination_key: format!("pr-for-validation-{}", issue.get()),
                    repos: vec![manifest_repo],
                }),
                action: Some("author_scenario".to_string()),
                checkout_capability: Some("writable".to_string()),
                allowed_verdicts: Vec::new(),
                verdict_contracts: Default::default(),
                source_metadata: Default::default(),
                guidance: None,
                structured_guidance: None,
                pull_request_freshness: None,
            },
        );
        let result = success_result(
            "worker-a",
            &job.job_id,
            &job.repo,
            "agent/scenario-author",
            "authored checked-in feature scenario",
        );
        let applier = ForgeApplier::new(
            forge.clone(),
            Arc::new(crate::breakdown_child_kind::plan_centric_workflow()),
        );

        applier.apply(job, result).await;

        let pulls = forge
            .list_pull_requests(&repo, PullRequestQuery::default())
            .await
            .expect("list scenario pull requests");
        assert_eq!(pulls.len(), 1);
        let pull = &pulls[0];
        assert_eq!(pull.source.branch, "agent/scenario-author");
        assert_eq!(pull.target.branch, "feature/scenario-lifecycle");
        assert_eq!(
            pull.labels,
            vec!["landing".to_string(), "scenario".to_string()]
        );
        let metadata = parse_metadata_block(&pull.body)
            .expect("scenario PR metadata parses")
            .expect("scenario PR metadata exists");
        assert_eq!(metadata.kind, Some(ArtifactKindId::new("scenario_pr")));
        assert_eq!(metadata.parents, vec![ArtifactRef::same_repo(issue)]);
    })
}
