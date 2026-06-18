// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

#[test]
fn engineer_decline_verdicts_route_issue_without_opening_pr() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());

        for (verdict, attention_label) in [
            ("needs_architect", "needs-architect"),
            ("needs_human", "needs-human"),
        ] {
            let issue = create_ready_issue(&forge, &repo).await;
            let job = open_pr_in_flight_job("acme/service", issue);
            let result = verdict_result("worker-a", &job.job_id, verdict, None);

            applier.apply(job, result).await;

            let labels = issue_labels(&forge, &repo, issue).await;
            assert!(has_label(&labels, "code"), "code label remains: {labels:?}");
            assert!(
                has_label(&labels, attention_label),
                "{verdict} should apply {attention_label}: {labels:?}"
            );
            assert!(!has_label(&labels, "ready"), "ready is cleared: {labels:?}");
            assert!(
                !has_label(&labels, "in-progress"),
                "working state is absent: {labels:?}"
            );
            assert_no_pull_requests(&forge, &repo).await;
        }

        assert_pull_request_count_stays(&cx, &forge, &repo, 0).await;
    })
}

#[test]
fn success_result_finalizes_source_issue_claim_state() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let root = MemoryForge::new();
        let repo = new_repo(&root, "stable").await;
        let issue = create_ready_issue(&root, &repo).await;
        let forge = Arc::new(root.as_user(role_user("engineer")));
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge, workflow);
        let job = open_pr_in_flight_job("acme/service", issue);
        let branch_name = format!("agent/pr-for-code-{}", issue.get());

        applier
            .apply(
                job.clone(),
                success_result(
                    "worker-a",
                    &job.job_id,
                    &job.repo,
                    &branch_name,
                    "implemented docs update",
                ),
            )
            .await;

        wait_for_pull_request_count(&cx, &root, &repo, 1).await;
        let issue = root
            .get_issue_by_number(&repo, issue)
            .await
            .expect("issue lookup succeeds")
            .expect("issue exists");
        assert_eq!(issue.labels, vec!["code".to_string()]);
        assert_eq!(issue.assignees, vec![UserId::new("engineer")]);
    })
}

#[test]
fn success_result_creates_implementation_pr_and_replay_is_idempotent() {
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
                &register("worker-a", "engineer", "acme/service")
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
        let summary = "implemented daemon worker success apply";
        let branch_name = format!("agent/pr-for-code-{}", issue.get());
        let posted_result = success_result(
            "worker-a",
            &assignment.job_id,
            &assignment.repo,
            &branch_name,
            summary,
        );
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(posted_result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let pull = &pulls[0];
        assert_eq!(
            pull.title,
            format!("Implement #{}: ready code issue", issue.get())
        );
        assert_eq!(pull.source.repository_id, repo);
        assert_eq!(pull.source.branch, branch_name);
        assert_eq!(pull.target.repository_id, repo);
        assert_eq!(pull.target.branch, "stable");
        assert!(pull.assignees.is_empty());
        assert_eq!(
            pull.labels,
            vec!["implementation".to_string(), "needs-reviewer".to_string()]
        );
        assert!(pull.body.contains(summary));
        assert!(!pull.body.contains("Implementation plan"));
        assert!(!pull.body.contains("- [ ]"));

        let pull_number = pull.number;
        let metadata = parse_metadata_block(&pull.body)
            .expect("PR metadata parses")
            .expect("PR metadata exists");
        assert_eq!(
            metadata.kind,
            Some(ArtifactKindId::new("implementation_pr"))
        );
        assert_eq!(metadata.parents, vec![ArtifactRef::same_repo(issue)]);
        let expected_correlation_key = format!("pr-for-code-{}", issue.get());
        assert_eq!(
            metadata.correlation_key.as_deref(),
            Some(expected_correlation_key.as_str())
        );

        drop_pull_request_label(&forge, &repo, pull_number, "needs-reviewer").await;
        assert_eq!(
            pull_request_labels(&forge, &repo, pull_number).await,
            vec!["implementation".to_string()]
        );

        let replay_job = InFlightJob {
            job_id: assignment.job_id.clone(),
            role: assignment.role.clone(),
            repo: assignment.repo.clone(),
            artifact: assignment.artifact.clone(),
            job_payload: assignment.job_payload.clone(),
        };
        let replay_result = success_result(
            "worker-a",
            &assignment.job_id,
            &assignment.repo,
            &branch_name,
            summary,
        );
        ForgeApplier::new(forge.clone(), workflow.clone())
            .apply(replay_job, replay_result)
            .await;
        assert_pull_request_count_stays(&cx, &forge, &repo, 1).await;
        assert_eq!(
            pull_request_labels(&forge, &repo, pull_number).await,
            vec!["implementation".to_string()]
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
                .expect("repeat feed succeeds and skips issue with an open implementation PR"),
            0
        );

        assert_pull_request_count_stays(&cx, &forge, &repo, 1).await;
    })
}

#[test]
fn success_result_with_multi_phase_plan_details_creates_checklist_body() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let job = open_pr_in_flight_job("acme/service", issue);
        let branch_name = format!("agent/pr-for-code-{}", issue.get());
        let summary = "implemented with a visible plan";
        let mut result = success_result("worker-a", &job.job_id, &job.repo, &branch_name, summary);
        result.details = Some(json!({
            "note": "fake worker result",
            "plan": {"phases": ["Write failing test", "Implement fix"]}
        }));

        applier.apply(job, result).await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let body = &pulls[0].body;
        assert!(body.contains("Summary: implemented with a visible plan"));
        assert!(
            body.contains("Implementation plan:\n\n- [ ] Write failing test\n- [ ] Implement fix")
        );
        assert!(
            body.find("Implementation plan") < body.find("<!-- temper:workflow"),
            "plan checklist should render before metadata block: {body}"
        );
        parse_metadata_block(body)
            .expect("PR metadata parses")
            .expect("PR metadata exists");
    })
}

#[test]
fn success_result_with_trivial_plan_details_keeps_plain_body() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let job = open_pr_in_flight_job("acme/service", issue);
        let branch_name = format!("agent/pr-for-code-{}", issue.get());
        let mut result = success_result(
            "worker-a",
            &job.job_id,
            &job.repo,
            &branch_name,
            "implemented one small edit",
        );
        result.details = Some(json!({"plan": {"phases": ["Apply obvious edit"]}}));

        applier.apply(job, result).await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let body = &pulls[0].body;
        assert!(body.contains("Summary: implemented one small edit"));
        assert!(!body.contains("Implementation plan"));
        assert!(!body.contains("- [ ]"));
    })
}

#[test]
fn coordinated_result_opens_one_pull_request_per_writable_repo() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        // Primary (home of the coordinating issue) + a second writable repo.
        let primary = create_repo(&forge, "acme", "service", "main").await;
        let secondary = create_repo(&forge, "acme", "lib", "main").await;
        let issue = create_ready_issue(&forge, &primary).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());

        let coordination_key = format!("coord-for-code-{}", issue.get());
        let branch = format!("agent/{coordination_key}");
        let job = coordinated_in_flight_job(
            "acme/service",
            issue,
            &coordination_key,
            vec![
                writable_repo("acme/service", &branch),
                writable_repo("acme/lib", &branch),
            ],
        );
        let result = JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-a".to_string(),
            job_id: job.job_id.clone(),
            status: ResultStatus::Success,
            repos: vec![
                RepoOutcome {
                    repo: "acme/service".to_string(),
                    branch: Branch {
                        name: branch.clone(),
                        head_sha: "aaa111".to_string(),
                    },
                },
                RepoOutcome {
                    repo: "acme/lib".to_string(),
                    branch: Branch {
                        name: branch.clone(),
                        head_sha: "bbb222".to_string(),
                    },
                },
            ],
            verdict: None,
            body: None,
            children: Vec::new(),
            failure: None,
            summary: Some("coordinated cross-repo change".to_string()),
            details: None,
        };

        applier.apply(job, result).await;

        // The primary repo's PR links back to the coordinating issue with a
        // bare same-repo ref, and carries the shared coordination key.
        let primary_pulls = forge
            .list_pull_requests(&primary, PullRequestQuery::default())
            .await
            .expect("list primary pull requests");
        assert_eq!(primary_pulls.len(), 1, "one PR opened in the primary repo");
        let primary_pull = &primary_pulls[0];
        assert_eq!(primary_pull.source.branch, branch);
        assert_eq!(primary_pull.target.branch, "main");
        let primary_meta = parse_metadata_block(&primary_pull.body)
            .expect("primary PR metadata parses")
            .expect("primary PR metadata exists");
        assert_eq!(primary_meta.parents, vec![ArtifactRef::same_repo(issue)]);
        assert_eq!(
            primary_meta.correlation_key.as_deref(),
            Some(coordination_key.as_str())
        );

        // The secondary repo's PR links to the SAME coordinating issue, but
        // repo-qualified to the primary repo — the cross-repo backref.
        let secondary_pulls = forge
            .list_pull_requests(&secondary, PullRequestQuery::default())
            .await
            .expect("list secondary pull requests");
        assert_eq!(
            secondary_pulls.len(),
            1,
            "one PR opened in the secondary repo"
        );
        let secondary_pull = &secondary_pulls[0];
        assert_eq!(secondary_pull.source.branch, branch);
        assert_eq!(secondary_pull.target.repository_id, secondary);
        let secondary_meta = parse_metadata_block(&secondary_pull.body)
            .expect("secondary PR metadata parses")
            .expect("secondary PR metadata exists");
        assert_eq!(
            secondary_meta.parents,
            vec![ArtifactRef::in_repo(primary.clone(), issue)]
        );
        assert_eq!(
            secondary_meta.correlation_key.as_deref(),
            Some(coordination_key.as_str())
        );
    })
}
