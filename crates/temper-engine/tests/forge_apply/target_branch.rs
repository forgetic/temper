// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::breakdown_child_kind::plan_centric_workflow;
use crate::support::*;

async fn create_policy_code_issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
    target_branch: &str,
) -> ItemNumber {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "feature branch implementation".to_string(),
                body: format!(
                    "Implement on the validated feature branch.\n\n{}",
                    render_metadata_block(&WorkflowMetadata {
                        target_branch: Some(target_branch.to_string()),
                        ..WorkflowMetadata::default()
                    })
                ),
                labels: vec!["code".to_string(), "ready".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("policy code issue is created")
        .number
}

fn policy_implementation_job(
    issue: ItemNumber,
    target_branch: &str,
    manifest_base: &str,
) -> InFlightJob {
    let head = format!("agent/pr-for-code-{}", issue.get());
    let mut manifest_repo = writable_repo("acme/service", &head);
    manifest_repo.default_branch = "main".to_string();
    manifest_repo.base_branch = manifest_base.to_string();
    let mut job = coordinated_in_flight_job(
        "acme/service",
        issue,
        &format!("pr-for-code-{}", issue.get()),
        vec![manifest_repo],
    );
    let mut context: JobContext =
        serde_json::from_value(job.job_payload.clone()).expect("job context parses");
    context.action = Some("open_pr".to_string());
    context
        .source_metadata
        .insert("target_branch".to_string(), target_branch.to_string());
    job.job_payload = serde_json::to_value(context).expect("job context serializes");
    job
}

#[test]
fn success_result_creates_implementation_pr_targeting_manifest_base_branch() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let branch_name = format!("agent/pr-for-code-{}", issue.get());
        let mut manifest_repo = writable_repo("acme/service", &branch_name);
        manifest_repo.default_branch = "stable".to_string();
        manifest_repo.base_branch = "feature/144-plan-branch".to_string();
        let job = coordinated_in_flight_job(
            "acme/service",
            issue,
            &format!("pr-for-code-{}", issue.get()),
            vec![manifest_repo],
        );

        applier
            .apply(
                job.clone(),
                success_result(
                    "worker-a",
                    &job.job_id,
                    &job.repo,
                    &branch_name,
                    "implemented feature branch targeting",
                ),
            )
            .await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        assert_eq!(pulls[0].target.branch, "feature/144-plan-branch");
        assert_eq!(pulls[0].source.branch, branch_name);
    })
}

#[test]
fn non_default_policy_targets_fresh_validated_feature_branch() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let target_branch = "agent/pr-for-feature-620";
        let issue = create_policy_code_issue(&forge, &repo, target_branch).await;
        let job = policy_implementation_job(issue, target_branch, target_branch);
        let head = format!("agent/pr-for-code-{}", issue.get());
        let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));

        applier
            .apply(
                job.clone(),
                success_result(
                    "worker-a",
                    &job.job_id,
                    &job.repo,
                    &head,
                    "implemented on the feature branch",
                ),
            )
            .await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        assert_eq!(pulls[0].source.branch, head);
        assert_eq!(pulls[0].target.branch, target_branch);
    })
}

#[test]
fn non_default_policy_rejects_existing_implementation_pr_with_default_target() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let target_branch = "agent/pr-for-feature-620";
        let issue = create_policy_code_issue(&forge, &repo, target_branch).await;
        let job = policy_implementation_job(issue, target_branch, target_branch);
        let head = format!("agent/pr-for-code-{}", issue.get());
        let original_body = "Existing implementation handoff must not be refreshed.";
        forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Existing implementation".to_string(),
                    body: original_body.to_string(),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: head.clone(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("divergent implementation PR is seeded");
        let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));

        applier
            .apply(
                job.clone(),
                success_result(
                    "worker-a",
                    &job.job_id,
                    &job.repo,
                    &head,
                    "must not replace the existing handoff",
                ),
            )
            .await;

        let pulls = forge
            .list_pull_requests(&repo, PullRequestQuery::default())
            .await
            .expect("pull requests list");
        assert_eq!(pulls.len(), 1);
        assert_eq!(pulls[0].target.branch, "main");
        assert_eq!(pulls[0].body, original_body);
        assert_eq!(pulls[0].labels, vec!["implementation".to_string()]);

        let labels = issue_labels(&forge, &repo, issue).await;
        assert!(has_label(&labels, "needs-human"), "labels: {labels:?}");
        assert!(has_label(&labels, "ready"), "labels: {labels:?}");
        assert!(!has_label(&labels, "in-progress"), "labels: {labels:?}");
        let comments = issue_comment_bodies(&forge, &repo, issue).await;
        assert_eq!(comments.len(), 1);
        assert!(comments[0].contains("branch topology diverges"));
        assert!(comments[0].contains(target_branch));
        assert!(comments[0].contains("main"));
    })
}

#[test]
fn non_default_policy_rejects_default_or_divergent_workspace_base() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        for manifest_base in ["main", "agent/pr-for-feature-999"] {
            let forge = Arc::new(MemoryForge::new());
            let repo = new_repo(&forge, "main").await;
            let target_branch = "agent/pr-for-feature-620";
            let issue = create_policy_code_issue(&forge, &repo, target_branch).await;
            let job = policy_implementation_job(issue, target_branch, manifest_base);
            let head = format!("agent/pr-for-code-{}", issue.get());
            let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));

            applier
                .apply(
                    job.clone(),
                    success_result(
                        "worker-a",
                        &job.job_id,
                        &job.repo,
                        &head,
                        "attempted a tampered target",
                    ),
                )
                .await;

            assert_no_pull_requests(&forge, &repo).await;
            let labels = issue_labels(&forge, &repo, issue).await;
            assert!(has_label(&labels, "needs-human"), "labels: {labels:?}");
            let comments = issue_comment_bodies(&forge, &repo, issue).await;
            assert_eq!(comments.len(), 1);
            assert!(comments[0].contains("diverges from fresh policy target"));
        }
    })
}

#[test]
fn non_default_policy_rejects_assignment_after_fresh_source_branch_changes() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let assigned_branch = "agent/pr-for-feature-620";
        let changed_branch = "agent/pr-for-feature-621";
        let issue = create_policy_code_issue(&forge, &repo, assigned_branch).await;
        let job = policy_implementation_job(issue, assigned_branch, assigned_branch);
        let issue_record = forge
            .get_issue_by_number(&repo, issue)
            .await
            .expect("issue lookup succeeds")
            .expect("issue exists");
        forge
            .update_issue(
                &issue_record.id,
                UpdateIssue {
                    body: Some(format!(
                        "Fresh metadata changed.\n\n{}",
                        render_metadata_block(&WorkflowMetadata {
                            target_branch: Some(changed_branch.to_string()),
                            ..WorkflowMetadata::default()
                        })
                    )),
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("source metadata changes");
        let head = format!("agent/pr-for-code-{}", issue.get());
        let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));

        applier
            .apply(
                job.clone(),
                success_result(
                    "worker-a",
                    &job.job_id,
                    &job.repo,
                    &head,
                    "attempted stale metadata target",
                ),
            )
            .await;

        assert_no_pull_requests(&forge, &repo).await;
        let comments = issue_comment_bodies(&forge, &repo, issue).await;
        assert_eq!(comments.len(), 1);
        assert!(comments[0].contains(changed_branch));
        assert!(comments[0].contains("diverges from fresh policy target"));
    })
}
