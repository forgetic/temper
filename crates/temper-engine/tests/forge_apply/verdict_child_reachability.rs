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
            pull_request_freshness: None,
        },
    )
}

#[test]
fn child_kind_without_a_serviceable_queue_is_rejected_before_mutation() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let plan = create_ready_plan(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));
        let job = decompose_plan_job("acme/service", plan);
        let before = issue_body_and_labels(&forge, &repo, plan).await;
        let mut child = job_child(
            "landing-regression",
            "Prove repaired-head CI blocks landing",
            "Add the integrated mechanical-landing regression.",
            &[],
        );
        child.kind = Some("validation".to_string());
        let result =
            verdict_result_with_children("worker-a", &job.job_id, "children_ready", vec![child]);

        applier.apply(job, result).await;

        let after = issue_body_and_labels(&forge, &repo, plan).await;
        assert_eq!(after.0, before.0);
        assert!(has_label(&after.1, "plan"));
        assert!(has_label(&after.1, "ready"));
        assert!(has_label(&after.1, "needs-human"));
        assert_eq!(list_issues(&forge, &repo).await.len(), 1);
        let comments = issue_comment_bodies(&forge, &repo, plan).await;
        assert_eq!(comments.len(), 1);
        assert!(
            comments[0].contains(
                "child `landing-regression` kind `validation` has no reachable workflow queue/action"
            ),
            "rejection evidence: {}",
            comments[0]
        );
    })
}
