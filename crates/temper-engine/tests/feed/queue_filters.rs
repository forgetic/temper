// SPDX-License-Identifier: MPL-2.0

use crate::support::*;

#[test]
fn scanned_role_work_skips_terminal_labeled_closed_issue() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_issue_record(&forge, &repo, &["code", "ready"]).await;
        forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    state: Some(IssueState::Closed),
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("issue is closed");
        let workflow = workflow();
        let compiled = workflow.compile();
        let role = RoleId::new("engineer");
        let (daemon, url, _rx) = spawn_recording(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();

        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-closed", "engineer", "acme/service")
            )
            .await
            .status,
            204
        );

        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    ts("2026-05-29T00:00:00Z"),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("feed succeeds and closed issue is skipped"),
            0
        );
        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-closed", 100)).await);
    })
}

#[test]
fn scanned_role_work_skips_item_when_enrichment_fails() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let _issue = create_issue(&forge, &repo, &["code", "ready"]).await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let role = RoleId::new("engineer");
        let (daemon, url, _rx) = spawn_recording(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();

        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-skip", "engineer", "acme/service")
            )
            .await
            .status,
            204
        );
        forge.fail_next(FaultOp::GetIssueByNumber, "issue snapshot lookup failed");

        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    ts("2026-05-29T00:00:00Z"),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("scan succeeds and enrichment failure is skipped"),
            0
        );
        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-skip", 100)).await);
    })
}

#[test]
fn scanned_writable_issue_skips_while_open_pr_has_correlation_key() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let role = RoleId::new("engineer");
        let correlation_key = format!("pr-for-code-{}", issue.get());
        let _pull_request =
            create_implementation_pull_request(&forge, &repo, &correlation_key).await;
        let (daemon, url, _rx) = spawn_recording(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();

        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-existing-pr", "engineer", "acme/service")
            )
            .await
            .status,
            204
        );

        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    ts("2026-05-29T00:00:00Z"),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("feed succeeds and open correlated PR is skipped"),
            0
        );
        assert_poll_timeout(
            post_json(&client, &url, &poll_with_wait("worker-existing-pr", 100)).await,
        );
    })
}

#[test]
fn scanned_writable_issue_skips_while_merged_pr_has_correlation_key() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let role = RoleId::new("engineer");
        let correlation_key = format!("pr-for-code-{}", issue.get());
        let pull_request =
            create_implementation_pull_request(&forge, &repo, &correlation_key).await;
        forge
            .merge_pull_request(
                &pull_request.id,
                MergePullRequest {
                    method: MergeMethod::Squash,
                    commit_title: None,
                    commit_body: None,
                },
            )
            .await
            .expect("pull request is merged");
        let (daemon, url, _rx) = spawn_recording(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();

        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-merged-pr", "engineer", "acme/service")
            )
            .await
            .status,
            204
        );

        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    ts("2026-05-29T00:00:00Z"),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("feed succeeds and merged correlated PR is skipped"),
            0
        );
        assert_poll_timeout(
            post_json(&client, &url, &poll_with_wait("worker-merged-pr", 100)).await,
        );
    })
}

#[test]
fn scanned_writable_issue_enqueues_after_correlated_pr_closes_unmerged() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let role = RoleId::new("engineer");
        let correlation_key = format!("pr-for-code-{}", issue.get());
        let pull_request =
            create_implementation_pull_request(&forge, &repo, &correlation_key).await;
        forge
            .update_pull_request(
                &pull_request.id,
                UpdatePullRequest {
                    state: Some(PullRequestUpdateState::Closed),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .expect("pull request is closed unmerged");
        let (daemon, url, _rx) = spawn_recording(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();

        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-closed-unmerged-pr", "engineer", "acme/service")
            )
            .await
            .status,
            204
        );

        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    ts("2026-05-29T00:00:00Z"),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("feed succeeds after correlated PR closes unmerged"),
            1
        );

        match post_json(&client, &url, &poll("worker-closed-unmerged-pr")).await {
            WorkerProtocolMessage::Assign(assign) => {
                assert_eq!(assign.role, "engineer");
                assert_eq!(assign.artifact.kind, "issue");
                assert_eq!(assign.artifact.item, json!(issue.get()));
            }
            other => panic!("expected assign after closing correlated PR unmerged, got {other:?}"),
        }
    })
}

#[test]
fn scanned_read_only_triage_item_enqueues_even_when_open_pr_exists() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_issue(&forge, &repo, &["untriaged"]).await;
        let _pull_request =
            create_implementation_pull_request(&forge, &repo, "pr-for-code-999").await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let role = RoleId::new("architect");
        let (daemon, url, _rx) = spawn_recording(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();

        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-triage-open-pr", "architect", "acme/service")
            )
            .await
            .status,
            204
        );

        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    ts("2026-05-29T00:00:00Z"),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("feed succeeds"),
            1
        );

        match post_json(&client, &url, &poll("worker-triage-open-pr")).await {
            WorkerProtocolMessage::Assign(assign) => {
                assert_eq!(assign.role, "architect");
                assert_eq!(assign.artifact.kind, "issue");
                assert_eq!(assign.artifact.item, json!(issue.get()));
                let context: temper_engine::JobContext =
                    serde_json::from_value(assign.job_payload).expect("triage payload parses");
                assert_eq!(context.checkout_capability.as_deref(), Some("read_only"));
            }
            other => panic!("expected triage assign, got {other:?}"),
        }
    })
}
