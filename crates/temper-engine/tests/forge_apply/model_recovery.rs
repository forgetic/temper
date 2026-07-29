// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

#[test]
fn authorized_model_rotation_uses_retry_claim_release_idempotently() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let root = MemoryForge::new();
        let repo = new_repo(&root, "stable").await;
        let issue_number = create_ready_issue(&root, &repo).await;
        let issue = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        root.update_issue(
            &issue.id,
            UpdateIssue {
                remove_labels: vec!["ready".to_string()],
                add_labels: vec!["in-progress".to_string()],
                add_assignees: vec![UserId::new("engineer")],
                ..UpdateIssue::default()
            },
        )
        .await
        .unwrap();
        let forge = Arc::new(root.as_user(role_user("engineer")));
        let applier = ForgeApplier::new(forge, Arc::new(workflow()));
        let job = open_pr_in_flight_job("acme/service", issue_number);
        let result = model_recovery_failure_result(
            "worker-a",
            &job.job_id,
            SessionRecoveryActionV1::RotateSession,
            1,
            1,
        );

        assert_eq!(
            applier.apply(job.clone(), result.clone()).await,
            temper_engine::ApplyOutcome::RetryReleased
        );
        assert_eq!(
            applier.apply(job, result).await,
            temper_engine::ApplyOutcome::RetryReleased
        );
        let released = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            released.labels,
            vec!["code".to_string(), "ready".to_string()]
        );
        assert!(released.assignees.is_empty());
        assert!(
            root.list_issue_comments(&released.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert_no_pull_requests(&root, &repo).await;
    })
}

#[test]
fn exhausted_model_recovery_parks_once_with_typed_actionable_audit() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        const SECRET: &str = "MODEL-PROMPT-SECRET-750";
        let root = MemoryForge::new();
        let repo = new_repo(&root, "stable").await;
        let issue_number = create_ready_issue(&root, &repo).await;
        let issue = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        root.update_issue(
            &issue.id,
            UpdateIssue {
                remove_labels: vec!["ready".to_string()],
                add_labels: vec!["in-progress".to_string()],
                add_assignees: vec![UserId::new("engineer")],
                ..UpdateIssue::default()
            },
        )
        .await
        .unwrap();
        let forge = Arc::new(root.as_user(role_user("engineer")));
        let job = open_pr_in_flight_job("acme/service", issue_number);
        let mut result = model_recovery_failure_result(
            "worker-a",
            &job.job_id,
            SessionRecoveryActionV1::ParkForHuman,
            7,
            1,
        );
        let diagnostic = result
            .failure
            .as_mut()
            .unwrap()
            .model_failure
            .as_mut()
            .unwrap();
        diagnostic.message = format!("Authorization: Bearer {SECRET}");
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));

        assert!(matches!(
            applier.apply(job.clone(), result.clone()).await,
            temper_engine::ApplyOutcome::Rejected {
                class: FailureClass::Permanent,
                ..
            }
        ));
        // A replacement applier must converge to the same marker and comment.
        let replacement = ForgeApplier::new(forge, Arc::new(workflow()));
        replacement.apply(job.clone(), result).await;

        assert_no_pull_requests(&root, &repo).await;
        let issue = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            issue.labels,
            vec!["code".to_string(), "needs-human".to_string()]
        );
        assert!(issue.assignees.is_empty());
        let comments = root.list_issue_comments(&issue.id).await.unwrap();
        assert_eq!(comments.len(), 1);
        let audit = &comments[0].body;
        for expected in [
            "failure_epoch: `7`",
            "cumulative_failure_count: `1`",
            "action: `park_for_human`",
            "session-fresh",
            "session-prior",
            ".temper-agent-session/state.json",
            "fixture-provider",
            "request-750",
            "disposition: `retryable`",
            "boundary: `http`",
            "Operator action:",
            "temper:comment-key=model_recovery_park:",
        ] {
            assert!(
                audit.contains(expected),
                "audit omitted {expected}: {audit}"
            );
        }
        for forbidden in [
            SECRET,
            "Authorization",
            "Bearer",
            "generic message must not be projected",
            "Provider failure details were redacted.",
        ] {
            assert!(!audit.contains(forbidden), "audit leaked {forbidden}");
        }
    })
}

#[test]
fn model_recovery_park_retries_partial_forge_convergence_without_duplicate_audit() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let root = MemoryForge::new();
        let repo = new_repo(&root, "stable").await;
        let issue_number = create_ready_issue(&root, &repo).await;
        let issue = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        root.update_issue(
            &issue.id,
            UpdateIssue {
                remove_labels: vec!["ready".to_string()],
                add_labels: vec!["in-progress".to_string()],
                add_assignees: vec![UserId::new("engineer")],
                ..UpdateIssue::default()
            },
        )
        .await
        .unwrap();
        let forge = Arc::new(root.as_user(role_user("engineer")));
        let applier = ForgeApplier::new(forge, Arc::new(workflow()));
        let job = open_pr_in_flight_job("acme/service", issue_number);
        let result = model_recovery_failure_result(
            "worker-a",
            &job.job_id,
            SessionRecoveryActionV1::ParkForHuman,
            3,
            1,
        );
        root.fail_next(FaultOp::AddIssueComment, "audit unavailable");

        assert!(matches!(
            applier.apply(job.clone(), result.clone()).await,
            temper_engine::ApplyOutcome::Retryable { .. }
        ));
        let partially_parked = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            partially_parked.labels,
            vec!["code".to_string(), "needs-human".to_string()]
        );
        assert!(partially_parked.assignees.is_empty());
        assert!(
            root.list_issue_comments(&partially_parked.id)
                .await
                .unwrap()
                .is_empty()
        );

        assert!(matches!(
            applier.apply(job.clone(), result.clone()).await,
            temper_engine::ApplyOutcome::Rejected { .. }
        ));
        applier.apply(job, result).await;
        let parked = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(root.list_issue_comments(&parked.id).await.unwrap().len(), 1);
    })
}

#[test]
fn model_recovery_park_releases_durable_assignment_and_cannot_be_rescanned() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let root = MemoryForge::new();
        let repo = new_repo(&root, "stable").await;
        let issue_number = create_ready_issue(&root, &repo).await;
        let forge = Arc::new(root.as_user(role_user("engineer")));
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
                    ts("2026-07-25T00:00:00Z"),
                    &RoleId::new("engineer"),
                    RoleFeedMode::Normal,
                )
                .await
                .unwrap(),
            1
        );
        let assignment = poll_assignment(&client, &url, "worker-a", issue_number).await;
        let result = model_recovery_failure_result(
            "worker-a",
            &assignment.job_id,
            SessionRecoveryActionV1::ParkForHuman,
            1,
            1,
        );
        let response = post(&client, &url, &WorkerProtocolMessage::Result(result)).await;
        assert_eq!(
            response.status, 422,
            "permanent park is terminally rejected"
        );

        let parked = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            parked.labels,
            vec!["code".to_string(), "needs-human".to_string()]
        );
        assert!(parked.assignees.is_empty());
        assert_durable_assignment_released(&parked);
        assert_eq!(root.list_issue_comments(&parked.id).await.unwrap().len(), 1);
        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo,
                    workflow.as_ref(),
                    &compiled,
                    ts("2026-07-25T00:01:00Z"),
                    &RoleId::new("engineer"),
                    RoleFeedMode::Normal,
                )
                .await
                .unwrap(),
            0
        );
        assert_no_pull_requests(&root, &repo).await;
    })
}
