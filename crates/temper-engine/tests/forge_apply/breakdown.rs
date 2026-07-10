// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

#[test]
fn breakdown_verdict_creates_children_across_repos() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo_a = new_repo(&forge, "stable").await;
        let repo_b = create_repo(&forge, "acme", "web", "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo_a).await;
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
        let role = RoleId::new("architect");

        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-a", "architect", "acme/service")
            )
            .await
            .status,
            204
        );

        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo_a,
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
        let assignment =
            poll_assignment_for_role(&client, &url, "worker-a", "architect", "issue", issue).await;
        let context: JobContext = serde_json::from_value(assignment.job_payload.clone())
            .expect("assignment payload is a JobContext");
        assert_eq!(context.action.as_deref(), Some("triage_intake"));
        assert_eq!(
            context.allowed_verdicts,
            vec!["needs_breakdown", "needs_design", "ready_code"]
        );
        assert_eq!(context.checkout_capability.as_deref(), Some("read_only"));

        let mut web = job_child(
            "web-client",
            "Implement the web client",
            "Build the web client against the API schema.",
            &["code", "ready"],
        );
        web.depends_on = vec!["api-schema".to_string()];
        web.target_repo = Some("acme/web".to_string());
        let result = verdict_result_with_children(
            "worker-a",
            &assignment.job_id,
            "needs_breakdown",
            vec![
                job_child(
                    "api-schema",
                    "Define the API schema",
                    "Write the shared API schema.",
                    &["code", "ready"],
                ),
                web,
            ],
        );
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let labels = issue_labels(&forge, &repo_a, issue).await;
            if !has_label(&labels, "untriaged") && has_label(&labels, "epic") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for breakdown verdict apply, saw labels {:?}",
                labels
            );
            temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(10)).await;
        }

        let repo_a_issues = wait_for_issue_count(&cx, &forge, &repo_a, 2).await;
        let repo_b_issues = wait_for_issue_count(&cx, &forge, &repo_b, 1).await;
        let api_child = issue_by_slug(&repo_a_issues, "api-schema");
        let web_child = issue_by_slug(&repo_b_issues, "web-client");

        assert_eq!(
            api_child.labels,
            vec!["code".to_string(), "ready".to_string()]
        );
        let api_metadata = parse_metadata_block(&api_child.body)
            .expect("api child metadata parses")
            .expect("api child metadata exists");
        assert_eq!(api_metadata.parents, vec![ArtifactRef::same_repo(issue)]);

        assert_eq!(
            web_child.labels,
            vec!["code".to_string(), "ready".to_string()]
        );
        let web_metadata = parse_metadata_block(&web_child.body)
            .expect("web child metadata parses")
            .expect("web child metadata exists");
        assert_eq!(
            web_metadata.parents,
            vec![ArtifactRef::in_repo(repo_a.clone(), issue)]
        );
        let expected_web_correlation_key =
            global_child_correlation_key(&repo_a, issue, "web-client");
        assert_eq!(
            web_metadata.correlation_key.as_deref(),
            Some(expected_web_correlation_key.as_str())
        );
        assert_eq!(
            web_metadata.dependencies,
            vec![ArtifactRef::in_repo(repo_a.clone(), api_child.number)]
        );

        let parent = forge
            .get_issue_by_number(&repo_a, issue)
            .await
            .expect("parent reload succeeds")
            .expect("parent exists");
        let parent_metadata = parse_metadata_block(&parent.body)
            .expect("parent metadata parses")
            .expect("parent metadata exists");
        assert_eq!(
            parent_metadata.dependencies,
            vec![
                ArtifactRef::in_repo(repo_a.clone(), api_child.number),
                ArtifactRef::in_repo(repo_b.clone(), web_child.number),
            ]
        );
    })
}

#[test]
fn breakdown_verdict_replay_is_idempotent() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo_a = new_repo(&forge, "stable").await;
        let repo_b = create_repo(&forge, "acme", "web", "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo_a).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);

        let mut web = job_child(
            "web-client",
            "Implement the web client",
            "Build the web client against the API schema.",
            &["code", "ready"],
        );
        web.depends_on = vec!["api-schema".to_string()];
        web.target_repo = Some("acme/web".to_string());
        let result = verdict_result_with_children(
            "worker-a",
            &job.job_id,
            "needs_breakdown",
            vec![
                job_child(
                    "api-schema",
                    "Define the API schema",
                    "Write the shared API schema.",
                    &["code", "ready"],
                ),
                web,
            ],
        );

        applier.apply(job.clone(), result.clone()).await;
        let repo_a_issues = list_issues(&forge, &repo_a).await;
        let repo_b_issues = list_issues(&forge, &repo_b).await;
        assert_eq!(repo_a_issues.len(), 2);
        assert_eq!(repo_b_issues.len(), 1);
        let api_child = issue_by_slug(&repo_a_issues, "api-schema");
        let web_child = issue_by_slug(&repo_b_issues, "web-client");
        let parent = forge
            .get_issue_by_number(&repo_a, issue)
            .await
            .expect("parent reload succeeds")
            .expect("parent exists");
        let parent_dependencies = parse_metadata_block(&parent.body)
            .expect("parent metadata parses")
            .expect("parent metadata exists")
            .dependencies;
        assert_eq!(
            parent_dependencies,
            vec![
                ArtifactRef::in_repo(repo_a.clone(), api_child.number),
                ArtifactRef::in_repo(repo_b.clone(), web_child.number),
            ]
        );

        applier.apply(job, result).await;

        assert_issue_count_stays(&cx, &forge, &repo_a, 2).await;
        assert_issue_count_stays(&cx, &forge, &repo_b, 1).await;
        let parent = forge
            .get_issue_by_number(&repo_a, issue)
            .await
            .expect("parent reload succeeds")
            .expect("parent exists");
        let after_replay_dependencies = parse_metadata_block(&parent.body)
            .expect("parent metadata parses")
            .expect("parent metadata exists")
            .dependencies;
        assert_eq!(after_replay_dependencies, parent_dependencies);
    })
}

#[test]
fn children_without_create_issues_effect_are_quarantined() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);
        let mut result = verdict_result(
            "worker-a",
            &job.job_id,
            "ready_code",
            Some("rewritten spec"),
        );
        result.children = vec![job_child(
            "stray-child",
            "Do not create me",
            "This child is not bound by the ready_code route.",
            &["code", "ready"],
        )];

        applier.apply(job, result).await;

        let (body, labels) = issue_body_and_labels(&forge, &repo, issue).await;
        assert_eq!(body, "rough user request");
        assert_eq!(
            labels,
            vec!["needs-human".to_string(), "untriaged".to_string()]
        );
        assert_eq!(list_issues(&forge, &repo).await.len(), 1);
        let comments = issue_comment_bodies(&forge, &repo, issue).await;
        assert_eq!(comments.len(), 1);
        assert!(comments[0].contains("requires exactly 0 child product(s)"));
    })
}

#[test]
fn unresolvable_child_target_repo_drops_apply() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo_a = new_repo(&forge, "stable").await;
        let repo_b = create_repo(&forge, "acme", "web", "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo_a).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);
        let mut child = job_child(
            "api-schema",
            "Define the API schema",
            "Write the shared API schema.",
            &["code", "ready"],
        );
        child.target_repo = Some("nobody/nowhere".to_string());
        let result =
            verdict_result_with_children("worker-a", &job.job_id, "needs_breakdown", vec![child]);

        applier.apply(job, result).await;

        let (body, labels) = issue_body_and_labels(&forge, &repo_a, issue).await;
        assert_eq!(body, "rough user request");
        assert_eq!(
            labels,
            vec!["needs-human".to_string(), "untriaged".to_string()]
        );
        assert_eq!(list_issues(&forge, &repo_a).await.len(), 1);
        assert!(list_issues(&forge, &repo_b).await.is_empty());
    })
}

#[test]
fn undeclared_verdict_does_not_mutate_issue() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = triage_in_flight_job("acme/service", issue);
        let before = issue_body_and_labels(&forge, &repo, issue).await;

        applier
            .apply(
                job.clone(),
                verdict_result("worker-a", &job.job_id, "nonsense", Some("rewritten spec")),
            )
            .await;

        let after = issue_body_and_labels(&forge, &repo, issue).await;
        assert_eq!(after.0, before.0);
        assert!(has_label(&after.1, "untriaged"));
        assert!(has_label(&after.1, "needs-human"));
        assert_no_pull_requests(&forge, &repo).await;
        let comments = issue_comment_bodies(&forge, &repo, issue).await;
        assert_eq!(comments.len(), 1);
        assert!(comments[0].contains("does not declare verdict `nonsense`"));
    })
}
