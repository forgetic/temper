// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;
use temper_protocol_worker::JobProgress;
use temper_workflow::{WorkflowMetadata, render_metadata_block};

#[test]
fn trivial_engineer_flow_stays_quiet_and_requests_review() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let compiled = workflow.compile();
        let applier = Arc::new(LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            Arc::new(ForgeApplier::new(forge.clone(), workflow.clone())),
            temper_engine::system_clock(),
        ));
        let daemon = Daemon::with_applier(Arc::new(handle.clone()), applier);
        let url = spawn(&handle, &daemon).await;
        let client = temper_engine_io::http::JsonClient::new();
        let role = RoleId::new("engineer");

        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-a", "engineer", "acme/service"),
            )
            .await
            .status,
            204
        );
        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo,
                    workflow.as_ref(),
                    &compiled,
                    ts("2026-05-29T00:00:00Z"),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("feed succeeds"),
            1
        );

        let assignment = poll_assignment(&client, &url, "worker-a", issue).await;
        let context: JobContext = serde_json::from_value(assignment.job_payload.clone())
            .expect("assignment payload is a JobContext");
        let workspace = context
            .workspace
            .as_ref()
            .expect("writable issue jobs carry a workspace manifest");
        let correlation = workspace.coordination_key.clone();
        assert_eq!(correlation, format!("pr-for-code-{}", issue.get()));
        let branch_name = workspace
            .primary()
            .and_then(|repo| repo.branch_hint.clone())
            .expect("primary writable repo has a branch hint");

        for progress in [
            flow_progress(&correlation, 1, "started", "start engineer run", None, None),
            flow_progress(
                &correlation,
                2,
                "started",
                "resume engineer run from pushed checkpoints",
                Some("fedcba98765432100123456789abcdef01234567"),
                None,
            ),
            flow_progress(
                &correlation,
                3,
                "done",
                "Apply obvious edit",
                Some("abc123456789"),
                Some("Single-phase checkpoint should stay off the issue thread."),
            ),
        ] {
            assert_eq!(
                post(&client, &url, &WorkerProtocolMessage::Progress(progress))
                    .await
                    .status,
                204
            );
        }
        assert_issue_comments_stay_empty(&cx, &forge, &repo, issue).await;

        let mut posted_result = success_result(
            "worker-a",
            &assignment.job_id,
            &assignment.repo,
            &branch_name,
            "implemented one obvious edit",
        );
        posted_result.details = Some(json!({
            "plan": {"phases": ["Apply obvious edit"]}
        }));
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(posted_result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let pull = wait_for_trivial_handoff(&cx, &forge, &repo).await;
        assert_eq!(
            pull.labels,
            vec!["implementation".to_string(), "needs-reviewer".to_string()]
        );
        assert!(!pull.labels.iter().any(|label| label == "in-progress"));
        assert_eq!(pull.requested_reviewers, vec![UserId::new("reviewer")]);
        assert!(pull.body.contains("Summary: implemented one obvious edit"));
        assert!(!pull.body.contains("Implementation plan"));
        assert!(!pull.body.contains("- [ ]"));

        let issue_after = forge
            .get_issue_by_number(&repo, issue)
            .await
            .expect("issue reload succeeds")
            .expect("issue exists");
        assert!(
            parse_metadata_block(&issue_after.body)
                .expect("issue metadata parses")
                .is_none_or(|metadata| metadata.lease.is_none()),
            "daemon lease metadata should be released from the source issue body: {}",
            issue_after.body
        );
        assert_issue_comments_stay_empty(&cx, &forge, &repo, issue).await;
    })
}

#[test]
fn existing_trivial_pr_with_working_label_gets_final_handoff() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let correlation = format!("pr-for-code-{}", issue.get());
        let branch_name = format!("agent/{correlation}");
        let body = format!(
            "Existing trivial implementation.\n\n{}",
            render_metadata_block(&WorkflowMetadata {
                kind: Some(ArtifactKindId::new("implementation_pr")),
                parents: vec![ArtifactRef::same_repo(issue)],
                correlation_key: Some(correlation),
                ..WorkflowMetadata::default()
            })
        );
        let existing = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: format!("Implement #{}: ready code issue", issue.get()),
                    body,
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: branch_name.clone(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "stable".to_string(),
                    },
                    labels: vec!["implementation".to_string(), "in-progress".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("existing PR is created");
        let job = open_pr_in_flight_job("acme/service", issue);
        let mut result = success_result(
            "worker-a",
            &job.job_id,
            &job.repo,
            &branch_name,
            "implemented one obvious edit",
        );
        result.details = Some(json!({"plan": {"phases": ["Apply obvious edit"]}}));

        applier.apply(job, result).await;

        assert_pull_request_count_stays(&cx, &forge, &repo, 1).await;
        let pull = forge
            .get_pull_request_by_number(&repo, existing.number)
            .await
            .expect("pull request lookup succeeds")
            .expect("pull request exists");
        assert_eq!(
            pull.labels,
            vec!["implementation".to_string(), "needs-reviewer".to_string()]
        );
        assert_eq!(pull.requested_reviewers, vec![UserId::new("reviewer")]);
        assert!(!pull.body.contains("Implementation plan"));
        assert_issue_comments_stay_empty(&cx, &forge, &repo, issue).await;
    })
}

async fn wait_for_trivial_handoff(
    cx: &temper_engine_io::Cx,
    forge: &MemoryForge,
    repo: &RepositoryId,
) -> PullRequest {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let pulls = forge
            .list_pull_requests(repo, PullRequestQuery::default())
            .await
            .expect("list pull requests succeeds");
        if pulls.len() == 1 {
            let pull = pulls[0].clone();
            if pull.labels == vec!["implementation".to_string(), "needs-reviewer".to_string()]
                && pull.requested_reviewers == vec![UserId::new("reviewer")]
            {
                return pull;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for trivial handoff PR, saw {pulls:?}"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

fn flow_progress(
    correlation_key: &str,
    step: u32,
    state: &str,
    status: &str,
    pushed_sha: Option<&str>,
    note: Option<&str>,
) -> JobProgress {
    JobProgress {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        correlation_key: correlation_key.to_string(),
        step,
        status: status.to_string(),
        state: state.to_string(),
        pushed_sha: pushed_sha.map(str::to_string),
        note: note.map(str::to_string),
        plan_publication: None,
    }
}
