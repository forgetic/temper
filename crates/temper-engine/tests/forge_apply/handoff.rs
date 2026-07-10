// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

#[test]
fn success_result_creates_implementation_pr_with_agent_authored_handoff() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let job = open_pr_in_flight_job("acme/service", issue);
        let branch_name = format!("agent/pr-for-code-{}", issue.get());
        let report = "# Implementation report\n\n- Added the durable handoff path.";
        let mut result = success_result(
            "worker-a",
            &job.job_id,
            &job.repo,
            &branch_name,
            "short log summary",
        );
        result.title = Some("Implement durable PR handoff".to_string());
        result.body = Some(report.to_string());

        applier.apply(job, result).await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let pull = &pulls[0];
        assert_eq!(pull.title, "Implement durable PR handoff");
        assert!(pull.body.starts_with(report));
        assert!(!pull.body.contains("Summary: short log summary"));
        let metadata = parse_metadata_block(&pull.body)
            .expect("PR metadata parses")
            .expect("PR metadata exists");
        assert_eq!(
            metadata.kind,
            Some(ArtifactKindId::new("implementation_pr"))
        );
        assert_eq!(metadata.parents, vec![ArtifactRef::same_repo(issue)]);
        assert_eq!(
            metadata.correlation_key.as_deref(),
            Some(format!("pr-for-code-{}", issue.get()).as_str())
        );
    })
}

#[test]
fn success_result_refreshes_existing_implementation_pr_handoff() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let job = open_pr_in_flight_job("acme/service", issue);
        let branch_name = format!("agent/pr-for-code-{}", issue.get());
        let metadata = WorkflowMetadata {
            kind: Some(ArtifactKindId::new("implementation_pr")),
            parents: vec![ArtifactRef::same_repo(issue)],
            correlation_key: Some(format!("pr-for-code-{}", issue.get())),
            ..WorkflowMetadata::default()
        };
        forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Old generated title".to_string(),
                    body: format!(
                        "Old report that must be replaced.\n\n{}",
                        render_metadata_block(&metadata)
                    ),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: branch_name.clone(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "stable".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("existing implementation PR exists");

        let new_report = "# Implementation report\n\nLatest compact handoff.";
        let mut result = success_result(
            "worker-a",
            &job.job_id,
            &job.repo,
            &branch_name,
            "latest summary",
        );
        result.title = Some("Implement refreshed handoff".to_string());
        result.body = Some(new_report.to_string());

        applier.apply(job, result).await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let pull = &pulls[0];
        assert_eq!(pull.title, "Implement refreshed handoff");
        assert!(pull.body.starts_with(new_report));
        assert!(!pull.body.contains("Old report that must be replaced"));
        assert_eq!(
            parse_metadata_block(&pull.body).expect("metadata parses"),
            Some(metadata)
        );
    })
}

#[test]
fn pull_request_writable_success_refreshes_same_pr_handoff_without_opening_another() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let branch_name = "agent/existing-pr";
        let metadata = WorkflowMetadata {
            kind: Some(ArtifactKindId::new("implementation_pr")),
            parents: vec![ArtifactRef::same_repo(issue)],
            correlation_key: Some(format!("pr-for-code-{}", issue.get())),
            ..WorkflowMetadata::default()
        };
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Old repair title".to_string(),
                    body: format!("Old repair report.\n\n{}", render_metadata_block(&metadata)),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: branch_name.to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "stable".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("existing PR exists");
        let job = pr_repair_in_flight_job("acme/service", pull_request.number, branch_name);
        let repair_report = "# Implementation report\n\nFixed the failing PR feedback.";
        let mut result = success_result(
            "worker-a",
            &job.job_id,
            &job.repo,
            branch_name,
            "fixed pr feedback",
        );
        result.title = Some("Refresh PR after feedback".to_string());
        result.body = Some(repair_report.to_string());

        applier.apply(job, result).await;

        let pulls = forge
            .list_pull_requests(&repo, PullRequestQuery::default())
            .await
            .expect("list pull requests");
        assert_eq!(pulls.len(), 1, "repair must not open a second PR");
        let pull = &pulls[0];
        assert_eq!(pull.number, pull_request.number);
        assert_eq!(pull.title, "Refresh PR after feedback");
        assert!(pull.body.starts_with(repair_report));
        assert!(!pull.body.contains("Old repair report"));
        assert_eq!(
            parse_metadata_block(&pull.body).expect("metadata parses"),
            Some(metadata)
        );
    })
}

fn pr_repair_in_flight_job(repo_path: &str, number: ItemNumber, branch: &str) -> InFlightJob {
    job_for_context(
        repo_path,
        number,
        "pull_request",
        JobContext {
            role: "engineer".to_string(),
            repo: repo_path.to_string(),
            queue: "pr_ci_failed".to_string(),
            artifact_kind: "implementation_pr".to_string(),
            artifact: None,
            workspace: Some(WorkspaceManifest {
                coordination_key: format!("pr-for-pull-request-{}", number.get()),
                repos: vec![writable_repo(repo_path, branch)],
            }),
            action: Some("address_ci_failure".to_string()),
            checkout_capability: Some("pull_request_writable".to_string()),
            allowed_verdicts: Vec::new(),
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: None,
            pull_request_freshness: None,
        },
    )
}
