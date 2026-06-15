// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

#[test]
fn triage_verdict_success_rewrites_body_and_routes_labels_without_pr() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
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

        let result = verdict_result(
            "worker-a",
            &assignment.job_id,
            "ready_code",
            Some("rewritten spec"),
        );
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let (body, labels) = loop {
            let state = issue_body_and_labels(&forge, &repo, issue).await;
            if state.0 == "rewritten spec" {
                break state;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for verdict apply, saw body {:?} labels {:?}",
                state.0,
                state.1
            );
            temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(10)).await;
        };

        assert_eq!(body, "rewritten spec");
        assert!(!labels.iter().any(|label| label == "untriaged"));
        assert!(labels.iter().any(|label| label == "code"));
        assert!(labels.iter().any(|label| label == "ready"));
        assert_no_pull_requests(&forge, &repo).await;
    })
}

#[test]
fn triage_verdict_replay_is_quiet_no_op() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = triage_in_flight_job("acme/service", issue);
        let result = verdict_result(
            "worker-a",
            &job.job_id,
            "ready_code",
            Some("rewritten spec"),
        );

        applier.apply(job.clone(), result.clone()).await;
        let after_first = issue_body_and_labels(&forge, &repo, issue).await;
        applier.apply(job, result).await;
        let after_second = issue_body_and_labels(&forge, &repo, issue).await;

        assert_eq!(after_first, after_second);
        assert_eq!(after_second.0, "rewritten spec");
        assert!(!after_second.1.iter().any(|label| label == "untriaged"));
        assert!(after_second.1.iter().any(|label| label == "code"));
        assert!(after_second.1.iter().any(|label| label == "ready"));
        assert_no_pull_requests(&forge, &repo).await;
    })
}
