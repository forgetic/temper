// SPDX-License-Identifier: MPL-2.0

use crate::support::*;

#[test]
fn scanned_architect_triage_item_carries_verdict_job_enrichment() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_issue(&forge, &repo, &["untriaged"]).await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let role = RoleId::new("architect");
        let (daemon, url, _rx) = spawn_recording(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();

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

        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-architect", "architect", "acme/service")
            )
            .await
            .status,
            204
        );

        let assignment = match post_json(&client, &url, &poll("worker-architect")).await {
            WorkerProtocolMessage::Assign(assign) => {
                assert_eq!(assign.repo, "acme/service");
                assert_eq!(assign.role, "architect");
                assert_eq!(assign.artifact.kind, "issue");
                assert_eq!(assign.artifact.item, json!(issue.get()));
                assert!(
                    assign
                        .job_id
                        .contains(&format!("/issue-{}/architect/triage", issue.get()))
                );
                assign
            }
            other => panic!("expected assign, got {other:?}"),
        };

        let context: temper_engine::JobContext = serde_json::from_value(assignment.job_payload)
            .expect("assign job payload parses as daemon-reexported JobContext");
        assert_eq!(context.role, "architect");
        assert_eq!(context.repo, "acme/service");
        assert_eq!(context.queue, "triage");
        assert_eq!(context.artifact_kind, "intake");
        assert_eq!(context.action.as_deref(), Some("triage_intake"));
        assert_eq!(context.checkout_capability.as_deref(), Some("read_only"));
        assert_eq!(context.allowed_verdicts, vec!["ready_code".to_string()]);
        let contract = context
            .verdict_contracts
            .get("ready_code")
            .expect("workflow-derived verdict contract is assigned");
        assert_eq!(contract.max_children, Some(0));
        assert!(contract.requires_body);
        let primary = context
            .workspace
            .as_ref()
            .expect("enriched job carries a workspace manifest")
            .primary()
            .expect("primary repo present");
        assert_eq!(primary.repo, "acme/service");
        assert_eq!(primary.base_branch, "main");
        let artifact = context.artifact.expect("issue snapshot is present");
        assert_eq!(artifact.number, issue.get());
        assert_eq!(artifact.labels, vec!["untriaged".to_string()]);
    })
}

#[test]
fn scanned_role_work_dispatches_to_worker_and_applies_once() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let role = RoleId::new("engineer");
        let (daemon, url, mut rx) = spawn_recording(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();

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
                .expect("repeat feed succeeds"),
            1
        );

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

        let assignment = match post_json(&client, &url, &poll("worker-a")).await {
            WorkerProtocolMessage::Assign(assign) => {
                assert_eq!(assign.repo, "acme/service");
                assert_eq!(assign.role, "engineer");
                assert_eq!(assign.artifact.kind, "issue");
                assert_eq!(assign.artifact.item, json!(issue.get()));
                assert!(
                    assign
                        .job_id
                        .contains(&format!("/issue-{}/engineer/", issue.get()))
                );
                assign
            }
            other => panic!("expected assign, got {other:?}"),
        };
        let context: temper_engine::JobContext =
            serde_json::from_value(assignment.job_payload.clone())
                .expect("assign job payload parses as daemon-reexported JobContext");
        assert_eq!(context.role, "engineer");
        assert_eq!(context.repo, "acme/service");
        assert_eq!(context.queue, "code_ready");
        assert_eq!(context.artifact_kind, "code");
        assert_eq!(context.action.as_deref(), Some("open_pr"));
        assert_eq!(context.checkout_capability.as_deref(), Some("writable"));
        assert!(context.allowed_verdicts.is_empty());
        // A non-coordinated issue gets a degenerate single-repo manifest.
        let expected_branch_hint = format!("agent/pr-for-code-{}", issue.get());
        let expected_coordination_key = format!("pr-for-code-{}", issue.get());
        let workspace = context
            .workspace
            .as_ref()
            .expect("enriched job carries a workspace manifest");
        assert_eq!(workspace.coordination_key, expected_coordination_key);
        assert_eq!(workspace.repos.len(), 1);
        let primary = workspace.primary().expect("primary repo present");
        assert_eq!(primary.repo, "acme/service");
        assert_eq!(primary.dir, "service");
        assert!(primary.is_writable());
        assert_eq!(primary.default_branch, "main");
        assert_eq!(primary.base_branch, "main");
        assert_eq!(
            primary.branch_hint.as_deref(),
            Some(expected_branch_hint.as_str())
        );
        let artifact = context.artifact.expect("issue snapshot is present");
        assert_eq!(artifact.number, issue.get());
        assert_eq!(artifact.title, "ready code issue");
        assert_eq!(artifact.body, "Implement the queued daemon work item.");
        assert_eq!(
            artifact.labels,
            vec!["code".to_string(), "ready".to_string()]
        );
        assert_eq!(artifact.state, "Open");

        let posted_result = job_result("worker-a", &assignment.job_id);
        match post_json(
            &client,
            &url,
            &WorkerProtocolMessage::Result(posted_result.clone()),
        )
        .await
        {
            WorkerProtocolMessage::Release(release) => {
                assert_eq!(release.worker_id, "worker-a");
                assert_eq!(release.job_id, assignment.job_id);
                assert_eq!(release.disposition, ReleaseDisposition::Accepted);
            }
            other => panic!("expected release, got {other:?}"),
        }

        let (job, recorded_result) = rx.recv().await.expect("applier records accepted result");
        assert_eq!(job.job_id, assignment.job_id);
        assert_eq!(job.repo, "acme/service");
        assert_eq!(job.role, "engineer");
        assert_eq!(job.artifact.kind, "issue");
        assert_eq!(job.artifact.item, json!(issue.get()));
        assert_eq!(job.job_payload, assignment.job_payload);
        assert_eq!(recorded_result, posted_result);
        assert_eq!(recorded_result.status, ResultStatus::Success);
        assert!(rx.try_recv().is_none());

        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 100)).await);
        assert!(rx.try_recv().is_none());
    })
}
