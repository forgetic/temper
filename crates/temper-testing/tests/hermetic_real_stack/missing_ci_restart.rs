use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use temper_forge_memory::FaultOp;
use temper_forge_model::{
    CiJobConclusion, CiJobQuery, CiJobStatus, CreatePullRequestReview, Forge, PullRequest,
    PullRequestState, RequestReviewers, ReviewDecision, UpdatePullRequest, UserId,
};
use temper_protocol_worker::ResultStatus;
use temper_testing::real_stack::{
    HermeticIssueSpec, HermeticRealStackBuilder, PausePoint, Reply, Script, StopReason, Turn,
};
use temper_workflow::parse_metadata_block;

#[test]
fn missing_repaired_head_ci_parks_once_across_restarts_and_blocks_landing_until_cleared() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        const MISSING_GRACE: Duration = Duration::from_secs(30);

        let sessions = Arc::new(AtomicUsize::new(0));
        let builder = missing_ci_stack_builder()
            .issue(HermeticIssueSpec::ready_code(
                "Restart missing repaired-head CI",
                "Repair a failed implementation PR, then tolerate a missing replacement run.",
            ))
            .fake_model_script(numbered_write_script(
                Arc::clone(&sessions),
                "service/MISSING_CI_REPAIR.md",
                "initial implementation\n",
                "repaired implementation\n",
            ))
            .ci_missing_grace(MISSING_GRACE);
        let mut stack = builder
            .build(&handle)
            .await
            .expect("missing-CI recovery world builds");

        let initial = stack
            .run_open_pr_job(&cx, &handle)
            .await
            .expect("initial implementation");
        assert_eq!(
            initial.job_result.status,
            ResultStatus::Success,
            "initial implementation failed: {:?}",
            initial.job_result.failure
        );
        stack.crash_worker().await;
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());

        let mut pull = initial.pull_requests[0].clone();
        let failed_head = initial.job_result.repos[0].branch.head_sha.clone();
        pull = stack
            .forge()
            .set_pull_request_head(&pull.id, Some(failed_head.clone()))
            .expect("seed failed head A");
        pull = stack
            .forge()
            .update_pull_request(
                &pull.id,
                UpdatePullRequest {
                    add_labels: vec!["landing".to_string(), "merge-conflict".to_string()],
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .expect("route conflict repair");
        stack
            .forge()
            .request_pull_request_reviewers(
                &pull.id,
                RequestReviewers {
                    reviewers: vec![UserId::new("reviewer")],
                },
            )
            .await
            .expect("request reviewer");
        stack
            .forge()
            .as_user(temper_testing::actor_user("reviewer"))
            .submit_pull_request_review(
                &pull.id,
                CreatePullRequestReview {
                    decision: ReviewDecision::Approved,
                    body: None,
                },
            )
            .await
            .expect("approve PR");
        stack
            .seed_ci_for_head(
                pull.number,
                failed_head.clone(),
                CiJobStatus::Completed,
                Some(CiJobConclusion::Failure),
            )
            .await
            .expect("retain terminal failure for A");

        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("repair enqueue"),
            1
        );
        let pushed_pause = stack.pause_hooks().arm(PausePoint::WorkerPushCompleted);
        let repair_session_pause = stack.pause_hooks().arm(PausePoint::AgentSessionStarted);
        stack.start_worker(&handle);
        repair_session_pause.arrived().await.release();
        let pushed = pushed_pause.arrived().await;
        let repaired_head = stack
            .origin_rev(stack.primary_repo_path(), &pull.source.branch)
            .expect("repair B pushed");
        assert_ne!(repaired_head, failed_head);
        stack
            .forge()
            .set_pull_request_head(&pull.id, Some(repaired_head.clone()))
            .expect("Forge observes repaired head B");

        // Replace both daemon-local monitor history and its Forge-facing wake
        // executor cache while A's failed job remains the only CI evidence.
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        let recovered = current_pull_request(&stack, pull.number).await;
        let metadata = parse_metadata_block(&recovered.body).unwrap().unwrap();
        assert_eq!(
            metadata.repaired_head.as_deref(),
            Some(repaired_head.as_str())
        );
        assert!(metadata.assignment.is_none() && metadata.lease.is_none());
        assert!(
            recovered
                .labels
                .iter()
                .any(|label| label == "implementation")
        );
        assert!(recovered.labels.iter().any(|label| label == "landing"));
        assert!(!recovered.labels.iter().any(|label| label == "needs-human"));
        let preserved_workflow_labels = recovered.labels.clone();

        assert_eq!(
            stack
                .run_ci_status_monitor_cadence()
                .await
                .expect("first missing observation"),
            0
        );
        stack.clock().advance(chrono::Duration::seconds(29));
        assert_eq!(
            stack
                .run_ci_status_monitor_cadence()
                .await
                .expect("pre-expiry observation"),
            0
        );
        assert_eq!(
            stack
                .reconcile_targeted_ci_mechanical(pull.number)
                .await
                .expect("pre-expiry targeted mechanical"),
            temper_runner::Progress::unchanged()
        );
        assert_eq!(
            stack
                .reconcile_startup_mechanical()
                .await
                .expect("pre-expiry broad mechanical"),
            temper_runner::Progress::unchanged()
        );
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("pre-expiry role scan"),
            0
        );
        assert_pre_expiry_state(&stack, pull.number).await;
        assert_eq!(sessions.load(Ordering::SeqCst), 2);

        stack.clock().advance(chrono::Duration::seconds(2));
        stack.forge().fail_next(
            FaultOp::AddPullRequestComment,
            "interrupt missing-CI parking after its durable barrier",
        );
        assert_eq!(
            stack
                .run_ci_status_monitor_cadence()
                .await
                .expect("expired missing observation"),
            1
        );
        let interrupted =
            wait_for_pull_request_label(&stack, &cx, pull.number, "needs-human").await;
        let interrupted_metadata = parse_metadata_block(&interrupted.body).unwrap().unwrap();
        let interrupted_recovery = interrupted_metadata
            .missing_ci_recovery
            .as_ref()
            .expect("interrupted parking retains a durable operation marker");
        assert_eq!(interrupted_recovery.head_sha, repaired_head);
        assert!(
            stack
                .forge()
                .list_pull_request_comments(&interrupted.id)
                .await
                .expect("interrupted missing-CI comments")
                .is_empty()
        );
        assert_eq!(
            stack
                .reconcile_targeted_ci_mechanical(pull.number)
                .await
                .expect("interrupted targeted mechanical is barred"),
            temper_runner::Progress::unchanged()
        );
        assert_eq!(
            stack
                .reconcile_startup_mechanical()
                .await
                .expect("interrupted broad mechanical is barred"),
            temper_runner::Progress::unchanged()
        );
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("interrupted role dispatch is barred"),
            0
        );

        // Replace the daemon and its ephemeral monitor after the attention
        // write but before the audit. The durable marker keeps this PR in the
        // narrow CI snapshot without making it eligible for role or mechanical
        // work, and a later bounded pass completes the same operation.
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        assert_eq!(
            stack
                .run_ci_status_monitor_cadence()
                .await
                .expect("replacement monitor observes interrupted parking"),
            0
        );
        stack.clock().advance(chrono::Duration::seconds(31));
        assert_eq!(
            stack
                .run_ci_status_monitor_cadence()
                .await
                .expect("replacement monitor retries interrupted parking"),
            1
        );
        wait_for_missing_ci_comment(&stack, &cx, &pull, &repaired_head).await;
        let parked = current_pull_request(&stack, pull.number).await;
        for label in &preserved_workflow_labels {
            assert!(
                parked.labels.contains(label),
                "parking removed workflow label `{label}` from {:?}",
                parked.labels
            );
        }
        assert!(parked.labels.iter().any(|label| label == "implementation"));
        assert!(parked.labels.iter().any(|label| label == "landing"));
        assert!(!parked.labels.iter().any(|label| label == "merge-conflict"));
        let parked_metadata = parse_metadata_block(&parked.body).unwrap().unwrap();
        assert_eq!(
            parked_metadata.repaired_head.as_deref(),
            Some(repaired_head.as_str())
        );
        assert!(parked_metadata.missing_ci_recovery.is_none());
        assert!(parked_metadata.assignment.is_none() && parked_metadata.lease.is_none());
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("parked role scan"),
            0
        );
        assert_eq!(sessions.load(Ordering::SeqCst), 2);

        // Repeated passes and a fresh daemon/monitor incarnation must retain
        // one label and one B-keyed audit without making the parked PR actionable again.
        assert_eq!(
            stack
                .run_ci_status_monitor_cadence()
                .await
                .expect("completed parking leaves the monitor snapshot"),
            0
        );
        assert_eq!(
            stack
                .reconcile_targeted_ci_mechanical(pull.number)
                .await
                .expect("parked targeted pass"),
            temper_runner::Progress::unchanged()
        );
        assert_eq!(
            stack
                .reconcile_startup_mechanical()
                .await
                .expect("parked broad pass"),
            temper_runner::Progress::unchanged()
        );
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        assert_eq!(
            stack
                .run_ci_status_monitor_cadence()
                .await
                .expect("replacement monitor starts grace"),
            0
        );
        stack.clock().advance(chrono::Duration::seconds(31));
        assert_eq!(
            stack
                .run_ci_status_monitor_cadence()
                .await
                .expect("replacement polling remains parked"),
            0
        );
        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(100)).await;
        assert_one_missing_ci_comment(&stack, &pull, &repaired_head).await;
        assert_eq!(sessions.load(Ordering::SeqCst), 2);

        pushed.release();
        let _ = stack
            .await_worker_result(&cx, Duration::from_secs(10))
            .await
            .expect("pre-restart worker result observed");
        stack
            .seed_ci_for_head(
                pull.number,
                repaired_head.clone(),
                CiJobStatus::Completed,
                Some(CiJobConclusion::Success),
            )
            .await
            .expect("delayed green B CI");
        let retained = stack
            .forge()
            .list_ci_jobs(stack.primary_repo_id(), CiJobQuery::default())
            .await
            .expect("retained CI inventory");
        assert!(retained.iter().any(|job| {
            job.commit_sha == failed_head && job.conclusion == Some(CiJobConclusion::Failure)
        }));
        assert!(retained.iter().any(|job| {
            job.commit_sha == repaired_head && job.conclusion == Some(CiJobConclusion::Success)
        }));

        assert_eq!(
            stack
                .reconcile_targeted_ci_mechanical(pull.number)
                .await
                .expect("delayed green targeted pass is parked"),
            temper_runner::Progress::unchanged()
        );
        assert_eq!(
            stack
                .reconcile_startup_mechanical()
                .await
                .expect("delayed green broad pass is parked"),
            temper_runner::Progress::unchanged()
        );
        assert_eq!(
            current_pull_request(&stack, pull.number).await.state,
            PullRequestState::Open
        );

        let parked = current_pull_request(&stack, pull.number).await;
        stack
            .forge()
            .update_pull_request(
                &parked.id,
                UpdatePullRequest {
                    remove_labels: vec!["needs-human".to_string()],
                    expected_version: Some(parked.version),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .expect("human clears attention barrier");
        assert!(
            stack
                .reconcile_targeted_ci_mechanical(pull.number)
                .await
                .expect("normal targeted landing resumes")
                .changed
        );
        assert_eq!(
            current_pull_request(&stack, pull.number).await.state,
            PullRequestState::Merged
        );
        assert_one_missing_ci_comment(&stack, &pull, &repaired_head).await;
        assert_eq!(sessions.load(Ordering::SeqCst), 2);
        stack.crash_worker().await;
    });
}

async fn current_pull_request(
    stack: &temper_testing::real_stack::HermeticRealStack,
    number: temper_forge_model::ItemNumber,
) -> PullRequest {
    stack
        .forge()
        .get_pull_request_by_number(stack.primary_repo_id(), number)
        .await
        .expect("pull request read")
        .expect("pull request exists")
}

async fn wait_for_pull_request_label(
    stack: &temper_testing::real_stack::HermeticRealStack,
    cx: &skein::cx::Cx,
    number: temper_forge_model::ItemNumber,
    label: &str,
) -> PullRequest {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pull_request = current_pull_request(stack, number).await;
        if pull_request
            .labels
            .iter()
            .any(|candidate| candidate == label)
        {
            return pull_request;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for pull request #{number} label `{label}`"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn assert_pre_expiry_state(
    stack: &temper_testing::real_stack::HermeticRealStack,
    number: temper_forge_model::ItemNumber,
) {
    let pull_request = current_pull_request(stack, number).await;
    assert!(
        !pull_request
            .labels
            .iter()
            .any(|label| label == "needs-human")
    );
    let metadata = parse_metadata_block(&pull_request.body).unwrap().unwrap();
    assert!(metadata.assignment.is_none() && metadata.lease.is_none());
    assert!(
        stack
            .forge()
            .list_pull_request_comments(&pull_request.id)
            .await
            .expect("pre-expiry comments")
            .is_empty()
    );
}

async fn wait_for_missing_ci_comment(
    stack: &temper_testing::real_stack::HermeticRealStack,
    cx: &skein::cx::Cx,
    pull: &PullRequest,
    repaired_head: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if !stack
            .forge()
            .list_pull_request_comments(&pull.id)
            .await
            .expect("missing-CI comments while waiting")
            .is_empty()
        {
            assert_one_missing_ci_comment(stack, pull, repaired_head).await;
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for missing-CI audit on pull request #{}",
            pull.number
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn assert_one_missing_ci_comment(
    stack: &temper_testing::real_stack::HermeticRealStack,
    pull: &PullRequest,
    repaired_head: &str,
) {
    let comments = stack
        .forge()
        .list_pull_request_comments(&pull.id)
        .await
        .expect("missing-CI comments");
    assert_eq!(comments.len(), 1);
    let body = &comments[0].body;
    assert!(body.contains(repaired_head));
    assert!(body.contains("matching `repaired_head`"));
    assert!(body.contains("no CI run or status for the current head"));
    assert!(body.contains("retrigger CI"));
    assert!(body.contains("clear `needs-human`"));
    assert!(body.contains(&format!("missing_current_head_ci:{repaired_head}")));
}

fn missing_ci_stack_builder() -> HermeticRealStackBuilder {
    let builder = HermeticRealStackBuilder::new();
    #[cfg(target_os = "linux")]
    let builder =
        builder.linux_supervisor_helper(env!("CARGO_BIN_EXE_temper-real-stack-supervisor-helper"));
    builder
}

fn numbered_write_script(
    sessions: Arc<AtomicUsize>,
    path: &'static str,
    first: &'static str,
    later: &'static str,
) -> Script {
    Script::rule(move |view| match view.prior_tool_results {
        0 => {
            let session = sessions.fetch_add(1, Ordering::SeqCst);
            Reply {
                turns: vec![Turn::ToolCall {
                    id: format!("write-session-{session}"),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": path,
                        "content": if session == 0 { first } else { later },
                    }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        }
        1 => Reply {
            turns: vec![Turn::ToolCall {
                id: "submit-numbered-session".to_string(),
                name: "submit_for_pr".to_string(),
                args: serde_json::json!({ "summary": "numbered session complete" }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        _ => Reply::text(
            serde_json::json!({
                "title": "Deterministic restart result",
                "body": "# Implementation report\nCompleted one deterministic restart session.",
                "summary": "Restart session complete."
            })
            .to_string(),
        ),
    })
}
