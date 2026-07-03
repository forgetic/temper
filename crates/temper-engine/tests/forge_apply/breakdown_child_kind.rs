// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

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
