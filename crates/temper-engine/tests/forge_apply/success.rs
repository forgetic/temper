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
fn stale_pr_guard_accepts_success_result_at_self_pushed_head() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let pull_request = create_guarded_pull_request(&forge, &repo).await;
        let assignment_head = "assigned-head";
        let self_head = "self-head";
        let pull_request = forge
            .set_pull_request_head(&pull_request.id, Some(assignment_head.to_string()))
            .expect("seed assignment head");
        forge
            .set_pull_request_head(&pull_request.id, Some(self_head.to_string()))
            .expect("advance to self head");
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let branch = "agent/pr-for-code-self";
        let job = pr_freshness_issue_job(
            "acme/service",
            issue,
            branch,
            pull_request_freshness(&repo, &pull_request, assignment_head),
        );
        let mut result = success_result("worker-a", &job.job_id, &job.repo, branch, "fixed CI");
        result.repos[0].branch.head_sha = self_head.to_string();

        applier.apply(job, result).await;

        let pulls = forge
            .list_pull_requests(&repo, PullRequestQuery::default())
            .await
            .expect("list pull requests");
        assert_eq!(pulls.len(), 2, "self-pushed result should not be dropped");
        assert!(pulls.iter().any(|pull| pull.source.branch == branch));
    })
}

#[test]
fn stale_pr_guard_drops_success_result_after_external_head_change() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let pull_request = create_guarded_pull_request(&forge, &repo).await;
        let assignment_head = "assigned-head";
        let self_head = "self-head";
        let pull_request = forge
            .set_pull_request_head(&pull_request.id, Some(assignment_head.to_string()))
            .expect("seed assignment head");
        forge
            .set_pull_request_head(&pull_request.id, Some("external-head".to_string()))
            .expect("advance to external head");
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let branch = "agent/pr-for-code-self";
        let job = pr_freshness_issue_job(
            "acme/service",
            issue,
            branch,
            pull_request_freshness(&repo, &pull_request, assignment_head),
        );
        let mut result = success_result("worker-a", &job.job_id, &job.repo, branch, "fixed CI");
        result.repos[0].branch.head_sha = self_head.to_string();

        applier.apply(job, result).await;

        let pulls = forge
            .list_pull_requests(&repo, PullRequestQuery::default())
            .await
            .expect("list pull requests");
        assert_eq!(pulls.len(), 1, "external head result should be dropped");
        assert!(!pulls.iter().any(|pull| pull.source.branch == branch));
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
fn success_result_finalizes_existing_branch_pr_even_when_correlation_lookup_misses() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let job = open_pr_in_flight_job("acme/service", issue);
        let branch_name = format!("agent/pr-for-code-{}", issue.get());

        forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: format!("Implement #{}: ready code issue", issue.get()),
                    body: "Existing implementation PR.\n\nSummary: pending".to_string(),
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
            .expect("existing branch PR exists before final success");

        let summary = "final implementation summary";
        applier
            .apply(
                job,
                success_result(
                    "worker-a",
                    "job-branch-fallback",
                    "acme/service",
                    &branch_name,
                    summary,
                ),
            )
            .await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let body = &pulls[0].body;
        assert!(body.contains("Summary: final implementation summary"));
        let metadata = parse_metadata_block(body)
            .expect("PR metadata parses")
            .expect("PR metadata exists");
        assert_eq!(metadata.parents, vec![ArtifactRef::same_repo(issue)]);
        assert_eq!(
            metadata.correlation_key.as_deref(),
            Some(format!("pr-for-code-{}", issue.get()).as_str())
        );
    })
}

#[test]
fn success_result_ignores_legacy_plan_details_for_plain_body() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let job = open_pr_in_flight_job("acme/service", issue);
        let branch_name = format!("agent/pr-for-code-{}", issue.get());
        let summary = "implemented with a final summary";
        let mut result = success_result("worker-a", &job.job_id, &job.repo, &branch_name, summary);
        result.details = Some(json!({
            "note": "fake worker result",
            "plan": {"phases": ["Write failing test", "Implement fix"]}
        }));

        applier.apply(job, result).await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let body = &pulls[0].body;
        assert!(body.contains("Summary: implemented with a final summary"));
        assert!(!body.contains("Implementation plan"));
        assert!(!body.contains("- [ ] Write failing test"));
        parse_metadata_block(body)
            .expect("PR metadata parses")
            .expect("PR metadata exists");
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
            title: None,
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

async fn create_guarded_pull_request(forge: &MemoryForge, repo: &RepositoryId) -> PullRequest {
    forge
        .create_pull_request(
            repo,
            CreatePullRequest {
                title: "Existing implementation".to_string(),
                body: "PR under repair".to_string(),
                source: BranchRef {
                    repository_id: repo.clone(),
                    branch: "agent/existing-pr".to_string(),
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
        .expect("pull request is created")
}

fn pull_request_freshness(
    repo: &RepositoryId,
    pull_request: &PullRequest,
    assignment_head: &str,
) -> temper_protocol_worker::PullRequestFreshness {
    temper_protocol_worker::PullRequestFreshness {
        repository_id: repo.as_str().to_string(),
        repo: "acme/service".to_string(),
        role: "engineer".to_string(),
        queue: "pr_ci_failed".to_string(),
        action: "address_ci_failure".to_string(),
        number: pull_request.number.get(),
        pull_request_id: pull_request.id.as_str().to_string(),
        head_sha: Some(assignment_head.to_string()),
        queue_condition: Some("ci_failed".to_string()),
        queue_labels: Vec::new(),
    }
}

fn pr_freshness_issue_job(
    repo_path: &str,
    number: ItemNumber,
    branch: &str,
    freshness: temper_protocol_worker::PullRequestFreshness,
) -> InFlightJob {
    job_for_context(
        repo_path,
        number,
        "issue",
        JobContext {
            role: "engineer".to_string(),
            repo: repo_path.to_string(),
            queue: "code_ready".to_string(),
            artifact_kind: "code".to_string(),
            artifact: None,
            workspace: Some(WorkspaceManifest {
                coordination_key: format!("pr-for-code-{}", number.get()),
                repos: vec![writable_repo(repo_path, branch)],
            }),
            action: Some("open_pr".to_string()),
            checkout_capability: Some("pull_request_writable".to_string()),
            allowed_verdicts: Vec::new(),
            guidance: None,
            pull_request_freshness: Some(freshness),
        },
    )
}
