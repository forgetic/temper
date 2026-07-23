// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::breakdown_child_kind::{
    create_feature_issue, plan_centric_workflow, plan_feature_in_flight_job,
};
use crate::support::*;

#[test]
fn needs_plan_without_target_branch_is_engine_stamped_before_mutation() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let issue = create_feature_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));
        let job = plan_feature_in_flight_job("acme/service", issue);
        let mut plan = job_child(
            "plan",
            "Plan the feature",
            "A prose-only plan whose branch is owned by Temper.",
            &[],
        );
        plan.kind = Some("plan".to_string());
        let result =
            verdict_result_with_children("worker-a", &job.job_id, "needs_plan", vec![plan]);

        applier.apply(job, result).await;

        let (_, labels) = issue_body_and_labels(&forge, &repo, issue).await;
        assert!(has_label(&labels, "planned"));
        assert!(!has_label(&labels, "needs-human"));
        let issues = list_issues(&forge, &repo).await;
        assert_eq!(issues.len(), 2);
        let plan = issue_by_slug(&issues, "plan");
        let metadata = parse_metadata_block(&plan.body)
            .expect("stamped metadata parses")
            .expect("stamped metadata exists");
        assert_eq!(
            metadata.target_branch.as_deref(),
            Some(format!("agent/pr-for-feature-{}", issue.get()).as_str())
        );
    })
}

#[test]
fn derived_policy_rejects_explicit_blank_default_and_divergent_branches_without_partial_mutation() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        for (branch, expected_reason) in [
            ("   ", "explicitly sets blank"),
            ("main", "repository default branch `main`"),
            ("feature/other", "expected `agent/pr-for-feature-1`"),
        ] {
            let forge = Arc::new(MemoryForge::new());
            let repo = new_repo(&forge, "main").await;
            let issue = create_feature_issue(&forge, &repo).await;
            let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));
            let job = plan_feature_in_flight_job("acme/service", issue);
            let body = format!(
                "Plan body.\n\n{}",
                render_metadata_block(&WorkflowMetadata {
                    target_branch: Some(branch.to_string()),
                    ..WorkflowMetadata::default()
                })
            );
            let mut plan = job_child("plan", "Plan the feature", &body, &[]);
            plan.kind = Some("plan".to_string());
            let result =
                verdict_result_with_children("worker-a", &job.job_id, "needs_plan", vec![plan]);

            applier.apply(job, result).await;

            let (body, labels) = issue_body_and_labels(&forge, &repo, issue).await;
            assert_eq!(body, "build the feature");
            assert!(has_label(&labels, "feature"));
            assert!(has_label(&labels, "needs-human"));
            assert!(!has_label(&labels, "planned"));
            assert_eq!(list_issues(&forge, &repo).await.len(), 1);
            let comments = issue_comment_bodies(&forge, &repo, issue).await;
            assert_eq!(comments.len(), 1);
            assert!(comments[0].contains(expected_reason), "{}", comments[0]);
        }
    })
}

#[test]
fn inherited_policy_stamps_omission_and_rejects_an_explicit_override() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        for explicit in [None, Some("main"), Some("agent/pr-for-feature-other")] {
            let forge = Arc::new(MemoryForge::new());
            let repo = new_repo(&forge, "main").await;
            let plan = forge
                .create_issue(
                    &repo,
                    CreateIssue {
                        title: "feature plan".to_string(),
                        body: format!(
                            "Implement the plan.\n\n{}",
                            render_metadata_block(&WorkflowMetadata {
                                kind: Some(ArtifactKindId::new("plan")),
                                target_branch: Some("agent/pr-for-feature-620".to_string()),
                                ..WorkflowMetadata::default()
                            })
                        ),
                        labels: vec!["plan".to_string(), "ready".to_string()],
                        assignees: Vec::new(),
                    },
                )
                .await
                .expect("plan issue is created")
                .number;
            let job = job_for_context(
                "acme/service",
                plan,
                "issue",
                JobContext {
                    trace_context: None,
                    artifact_context: None,
                    role: "architect".to_string(),
                    repo: "acme/service".to_string(),
                    queue: "plan_ready".to_string(),
                    artifact_kind: "plan".to_string(),
                    artifact: None,
                    workspace: None,
                    action: Some("decompose_plan".to_string()),
                    checkout_capability: Some("read_only".to_string()),
                    allowed_verdicts: vec!["children_ready".to_string()],
                    verdict_contracts: Default::default(),
                    source_metadata: Default::default(),
                    guidance: None,
                    structured_guidance: None,
                    pull_request_freshness: None,
                },
            );
            let child_body = explicit.map_or_else(
                || "Implement one child.".to_string(),
                |branch| {
                    format!(
                        "Implement one child.\n\n{}",
                        render_metadata_block(&WorkflowMetadata {
                            target_branch: Some(branch.to_string()),
                            ..WorkflowMetadata::default()
                        })
                    )
                },
            );
            let result = verdict_result_with_children(
                "worker-a",
                &job.job_id,
                "children_ready",
                vec![job_child("child", "Implement child", &child_body, &[])],
            );
            let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));

            applier.apply(job, result).await;

            let issues = list_issues(&forge, &repo).await;
            if explicit.is_none() {
                assert_eq!(issues.len(), 2);
                let child = issue_by_slug(&issues, "child");
                let metadata = parse_metadata_block(&child.body)
                    .expect("child metadata parses")
                    .expect("child metadata exists");
                assert_eq!(
                    metadata.target_branch.as_deref(),
                    Some("agent/pr-for-feature-620")
                );
            } else {
                assert_eq!(issues.len(), 1);
                let (_, labels) = issue_body_and_labels(&forge, &repo, plan).await;
                assert!(has_label(&labels, "ready"));
                assert!(has_label(&labels, "needs-human"));
                assert!(!has_label(&labels, "in-progress"));
            }
        }
    })
}
