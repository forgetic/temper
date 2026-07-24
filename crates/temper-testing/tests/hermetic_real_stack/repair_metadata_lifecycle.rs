use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use temper_forge_model::{
    CiJobConclusion, CiJobStatus, CreatePullRequestReview, Forge, PullRequest, PullRequestState,
    RequestReviewers, ReviewDecision, UpdatePullRequest, UserId,
};
use temper_protocol_worker::{JobResult, ResultStatus};
use temper_testing::real_stack::{
    HermeticIssueSpec, HermeticRealStack, HermeticRealStackBuilder, PausePoint, Reply, Script,
    StopReason, Turn,
};
use temper_workflow::{
    DurableAssignment, Lease, RoleId, WorkflowMetadata, inspect_metadata_blocks,
    parse_metadata_block, render_metadata_block,
};

const STALE_ASSIGNMENT_ID: &str = "sentinel-stale-assignment-job";
const STALE_ATTEMPT_ID: &str = "daemon-boot-18c4f5169b17c528-1-1";
const STALE_DAEMON_BOOT_ID: &str = "daemon-boot-18c4f5169b17c528";
const STALE_LEASE_OWNER: &str = "sentinel-stale-lease-owner";
const STALE_WORKER_ID: &str = "sentinel-stale-worker";
const STALE_COORDINATION_KEY: &str = "sentinel-stale-coordination";
const STALE_REPAIRED_HEAD: &str = "sentinel-stale-repaired-head";

#[test]
fn repair_metadata_stays_canonical_across_release_replay_and_daemon_replacement() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let sessions = Arc::new(AtomicUsize::new(0));
        let mut stack = repair_stack_builder()
            .issue(HermeticIssueSpec::ready_code(
                "Restart merge-conflict repair",
                "Open and then repair one implementation PR.",
            ))
            .fake_model_script(repair_metadata_write_script(Arc::clone(&sessions)))
            .build(&handle)
            .await
            .expect("repair world builds");

        let initial = stack
            .run_open_pr_job(&cx, &handle)
            .await
            .expect("initial implementation");
        assert_eq!(initial.job_result.status, ResultStatus::Success);
        wait_for_pending_result_count(&stack, &cx, 0).await;
        stack.crash_worker().await;
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        let mut pull = initial.pull_requests[0].clone();
        let canonical_identity = parse_metadata_block(&pull.body)
            .expect("initial implementation metadata parses")
            .expect("initial implementation metadata exists");
        let initial_head = initial.job_result.repos[0].branch.head_sha.clone();
        pull = stack
            .forge()
            .set_pull_request_head(&pull.id, Some(initial_head.clone()))
            .expect("seed initial head");
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
            .expect("route conflict");
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

        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("repair enqueue"),
            1
        );
        let pushed_pause = stack.pause_hooks().arm(PausePoint::WorkerPushCompleted);
        let applied_pause = stack
            .pause_hooks()
            .arm(PausePoint::ResultApplicationCompleted);
        let uncertain_delivery_pause = stack
            .pause_hooks()
            .arm(PausePoint::WorkerResultDeliveryResolved);
        let repair_session_pause = stack.pause_hooks().arm(PausePoint::AgentSessionStarted);
        stack.start_worker(&handle);
        repair_session_pause.arrived().await.release();
        let pushed = pushed_pause.arrived().await;
        let repaired_head = stack
            .origin_rev(stack.primary_repo_path(), &pull.source.branch)
            .expect("repair pushed");
        assert_ne!(repaired_head, initial_head);
        stack
            .forge()
            .set_pull_request_head(&pull.id, Some(repaired_head.clone()))
            .expect("observe repair head");
        stack
            .seed_ci_for_head(
                pull.number,
                repaired_head.clone(),
                CiJobStatus::Queued,
                None,
            )
            .await
            .expect("queue CI");

        // Publish the repaired head through the exact terminal result, then let
        // the ordinary lease decorator release assignment and lease ownership.
        pushed.release();
        let applied = applied_pause.arrived().await;
        let published = current_pull_request(&stack, pull.number).await;
        assert_canonical_repair_body(&published, &canonical_identity, &repaired_head);
        assert!(published.labels.iter().any(|label| label == "landing"));
        assert!(
            !published
                .labels
                .iter()
                .any(|label| label == "merge-conflict")
        );
        let body_after_release = published.body.clone();
        let version_after_release = published.version;
        applied.release();

        // Crash after daemon acceptance but before durable outbox compaction,
        // forcing the replacement worker to redeliver the exact stale report.
        let uncertain_delivery = uncertain_delivery_pause.arrived().await;
        let first_result = await_worker_result_for_head(&mut stack, &cx, &repaired_head).await;
        let raw_report = first_result.body.as_deref().expect("repair result report");
        for stale in stale_sentinels() {
            assert!(
                raw_report.contains(stale),
                "repair result did not contain stale bookkeeping sentinel: {stale}"
            );
        }
        assert!(raw_report.contains("Inline example: `<!-- temper:workflow {} -->`."));
        assert!(
            raw_report.contains("```text\n<!-- temper:workflow\n{\"kind\":\"example\"}\n-->\n```")
        );
        assert_eq!(
            inspect_metadata_blocks(raw_report)
                .expect("repair result report is structurally inspectable")
                .block_count(),
            1,
            "inline and fenced examples remain prose beside one real stale block"
        );
        stack.crash_worker().await;
        drop(uncertain_delivery);
        assert_eq!(
            stack.pending_result_count().expect("durable result outbox"),
            1
        );

        let replay_applied_pause = stack
            .pause_hooks()
            .arm(PausePoint::ResultApplicationCompleted);
        stack.start_worker(&handle);
        replay_applied_pause.arrived().await.release();
        let replayed_result = await_worker_result_for_head(&mut stack, &cx, &repaired_head).await;
        assert_eq!(replayed_result, first_result);
        wait_for_pending_result_count(&stack, &cx, 0).await;
        let replayed = current_pull_request(&stack, pull.number).await;
        assert_eq!(replayed.body, body_after_release);
        assert_eq!(replayed.version, version_after_release);
        assert_canonical_repair_body(&replayed, &canonical_identity, &repaired_head);

        // Production startup parsing must inventory the canonical state without
        // staging stale ownership or making the PR eligible for redispatch.
        stack.crash_worker().await;
        stack.replace_daemon_through_startup_recovery(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        let recovered = current_pull_request(&stack, pull.number).await;
        assert_eq!(recovered.body, body_after_release);
        assert_eq!(recovered.version, version_after_release);
        assert_canonical_repair_body(&recovered, &canonical_identity, &repaired_head);
        assert_eq!(
            stack
                .reconcile_startup_mechanical()
                .await
                .expect("queued pass"),
            temper_runner::Progress::unchanged()
        );
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("repair quiet"),
            0
        );
        assert_eq!(sessions.load(Ordering::SeqCst), 2);

        stack
            .seed_ci_for_head(
                pull.number,
                repaired_head,
                CiJobStatus::Completed,
                Some(CiJobConclusion::Success),
            )
            .await
            .expect("green CI");
        assert!(
            stack
                .reconcile_startup_mechanical()
                .await
                .expect("landing pass")
                .changed
        );
        let final_pull = current_pull_request(&stack, pull.number).await;
        assert_eq!(final_pull.state, PullRequestState::Merged);
        assert_eq!(stack.pull_requests().await.expect("PR inventory").len(), 1);
        assert_eq!(
            stack
                .reconcile_startup_mechanical()
                .await
                .expect("quiet pass"),
            temper_runner::Progress::unchanged()
        );
        assert_eq!(stack.persisted_session_count().expect("session count"), 1);
        stack.crash_worker().await;
    });
}

fn stale_sentinels() -> [&'static str; 7] {
    [
        STALE_ASSIGNMENT_ID,
        STALE_ATTEMPT_ID,
        STALE_DAEMON_BOOT_ID,
        STALE_LEASE_OWNER,
        STALE_WORKER_ID,
        STALE_COORDINATION_KEY,
        STALE_REPAIRED_HEAD,
    ]
}

async fn current_pull_request(
    stack: &HermeticRealStack,
    number: temper_forge_model::ItemNumber,
) -> PullRequest {
    stack
        .forge()
        .get_pull_request_by_number(stack.primary_repo_id(), number)
        .await
        .expect("pull request read")
        .expect("pull request exists")
}

fn assert_canonical_repair_body(
    pull: &PullRequest,
    canonical_identity: &WorkflowMetadata,
    repaired_head: &str,
) {
    let inspection = inspect_metadata_blocks(&pull.body).expect("published body is inspectable");
    assert_eq!(inspection.block_count(), 1);
    let metadata = parse_metadata_block(&pull.body)
        .expect("published metadata parses")
        .expect("published metadata exists");
    assert_eq!(metadata.repaired_head.as_deref(), Some(repaired_head));
    assert!(metadata.assignment.is_none());
    assert!(metadata.lease.is_none());
    assert_eq!(metadata.kind, canonical_identity.kind);
    assert_eq!(metadata.parents, canonical_identity.parents);
    assert_eq!(metadata.dependencies, canonical_identity.dependencies);
    assert_eq!(metadata.correlation_key, canonical_identity.correlation_key);
    assert_eq!(metadata.target_branch, canonical_identity.target_branch);
    for stale in stale_sentinels() {
        assert!(
            !pull.body.contains(stale),
            "stale result bookkeeping escaped into the published body: {stale}"
        );
    }
    assert!(
        pull.body
            .contains("Inline example: `<!-- temper:workflow {} -->`.")
    );
    assert!(
        pull.body
            .contains("```text\n<!-- temper:workflow\n{\"kind\":\"example\"}\n-->\n```")
    );
}

async fn await_worker_result_for_head(
    stack: &mut HermeticRealStack,
    cx: &skein::cx::Cx,
    expected_head: &str,
) -> JobResult {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = stack
            .await_worker_result(cx, remaining)
            .await
            .expect("worker result observed");
        if result.repos.iter().any(|outcome| {
            outcome.repo == stack.primary_repo_path() && outcome.branch.head_sha == expected_head
        }) {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for result that published head {expected_head}"
        );
    }
}

async fn wait_for_pending_result_count(
    stack: &HermeticRealStack,
    cx: &skein::cx::Cx,
    expected: usize,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let count = stack.pending_result_count().expect("durable result outbox");
        if count == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} pending result(s), saw {count}"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

fn repair_metadata_write_script(sessions: Arc<AtomicUsize>) -> Script {
    Script::rule(move |view| match view.prior_tool_results {
        0 => {
            let session = sessions.fetch_add(1, Ordering::SeqCst);
            Reply {
                turns: vec![Turn::ToolCall {
                    id: format!("write-repair-session-{session}"),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": "service/REPAIR.md",
                        "content": if session == 0 {
                            "initial implementation\n"
                        } else {
                            "merge conflict repaired\n"
                        },
                    }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        }
        1 => Reply {
            turns: vec![Turn::ToolCall {
                id: "submit-repair-session".to_string(),
                name: "submit_for_pr".to_string(),
                args: serde_json::json!({ "summary": "repair session complete" }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        _ => {
            let repair = sessions.load(Ordering::SeqCst) > 1;
            Reply::text(
                serde_json::json!({
                    "title": if repair {
                        "Repair canonical metadata lifecycle"
                    } else {
                        "Initial implementation"
                    },
                    "body": if repair {
                        stale_repair_report()
                    } else {
                        "# Implementation report\n\nCreated the initial implementation.".to_string()
                    },
                    "summary": if repair {
                        "Repaired the implementation PR."
                    } else {
                        "Created the initial implementation."
                    }
                })
                .to_string(),
            )
        }
    })
}

fn stale_repair_report() -> String {
    let stale_metadata = WorkflowMetadata {
        lease: Some(Lease {
            role: RoleId::new("engineer"),
            worker: STALE_LEASE_OWNER.to_string(),
            claimed_at: "2026-07-23T00:00:00Z"
                .parse()
                .expect("stale claimed timestamp"),
            heartbeat_at: "2026-07-23T00:01:00Z"
                .parse()
                .expect("stale heartbeat timestamp"),
            expires_at: "2026-07-23T00:30:00Z"
                .parse()
                .expect("stale expiry timestamp"),
        }),
        assignment: Some(DurableAssignment {
            job_id: Some(STALE_ASSIGNMENT_ID.to_string()),
            attempt_id: Some(STALE_ATTEMPT_ID.to_string()),
            role: Some(RoleId::new("engineer")),
            queue: Some("pr_ci_failed".to_string()),
            action: Some("address_ci_failure".to_string()),
            worker_id: Some(STALE_WORKER_ID.to_string()),
            coordination_key: Some(STALE_COORDINATION_KEY.to_string()),
            daemon_boot_id: Some(STALE_DAEMON_BOOT_ID.to_string()),
            ..DurableAssignment::default()
        }),
        repaired_head: Some(STALE_REPAIRED_HEAD.to_string()),
        ..WorkflowMetadata::default()
    };
    format!(
        "# Implementation report\n\nRepaired the implementation PR.\n\nInline example: `<!-- temper:workflow {{}} -->`.\n\n```text\n<!-- temper:workflow\n{{\"kind\":\"example\"}}\n-->\n```\n\n{}",
        render_metadata_block(&stale_metadata)
    )
}

fn repair_stack_builder() -> HermeticRealStackBuilder {
    let builder = HermeticRealStackBuilder::new();
    #[cfg(target_os = "linux")]
    let builder =
        builder.linux_supervisor_helper(env!("CARGO_BIN_EXE_temper-real-stack-supervisor-helper"));
    builder
}
