use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use temper_forge_model::{
    CiJobConclusion, CiJobStatus, CreateIssue, CreatePullRequestReview, Forge, IssueState,
    PullRequestState, RequestReviewers, ReviewDecision, UpdateIssue, UpdatePullRequest, UserId,
};
use temper_protocol_worker::ResultStatus;
use temper_testing::real_stack::{
    FakeModelResponse, HermeticIssueSpec, HermeticRealStackBuilder, PausePoint, Reply, Script,
    StopReason, Turn,
};
use temper_workflow::parse_metadata_block;

#[test]
fn dirty_workspace_replays_after_target_advance_and_component_replacement() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let mut stack = HermeticRealStackBuilder::new()
            .issue(HermeticIssueSpec::ready_code(
                "Recover dirty workspace",
                "Preserve interrupted tracked and untracked edits.\n\n<!-- temper:workflow\n{\"kind\":\"code\"}\n-->",
            ))
            .fake_model_response(FakeModelResponse::write_file(
                "service/RECOVERED.md",
                "agent resumed\n",
                "Recovered the interrupted workspace.",
            ))
            .build(&handle)
            .await
            .expect("dirty recovery world builds");

        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("source enqueues"),
            1
        );
        let session_pause = stack.pause_hooks().arm(PausePoint::AgentSessionStarted);
        stack.start_worker(&handle);
        let session = session_pause.arrived().await;
        let checkout = stack
            .workspace_checkout(stack.primary_repo_path())
            .expect("prepared checkout");
        fs::write(checkout.join("README.md"), "interrupted tracked edit\n").expect("tracked edit");
        fs::write(
            checkout.join("UNTRACKED.txt"),
            "interrupted untracked edit\n",
        )
        .expect("untracked edit");
        stack
            .advance_origin_branch(
                stack.primary_repo_path(),
                "main",
                "TARGET_ADVANCED.txt",
                "target advanced while Temper was offline\n",
            )
            .expect("advance target");

        stack.crash_worker().await;
        session.release();
        stack.replace_daemon(&handle).await;
        assert_eq!(stack.open_recovery_barrier().await.len(), 1);
        let run = stack
            .run_open_pr_job(&cx, &handle)
            .await
            .expect("replacement worker replays");
        assert_eq!(run.job_result.status, ResultStatus::Success);
        assert_eq!(run.pull_requests.len(), 1);
        let branch = &run.job_result.repos[0].branch.name;
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), branch, "README.md")
                .unwrap(),
            "interrupted tracked edit\n"
        );
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), branch, "UNTRACKED.txt")
                .unwrap(),
            "interrupted untracked edit\n"
        );
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), branch, "TARGET_ADVANCED.txt")
                .unwrap(),
            "target advanced while Temper was offline\n"
        );
        assert_eq!(stack.persisted_session_count().expect("session count"), 1);
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("quiet scan"),
            0
        );
        stack.crash_worker().await;
    });
}

#[test]
fn matching_worker_heartbeat_reattaches_exact_durable_job_once() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let sessions = Arc::new(AtomicUsize::new(0));
        let mut stack = HermeticRealStackBuilder::new()
            .issue(HermeticIssueSpec::ready_code(
                "Reattach exact heartbeat",
                "Keep one active coding session across daemon replacement.\n\n<!-- temper:workflow\n{\"kind\":\"code\"}\n-->",
            ))
            .fake_model_script(numbered_write_script(
                Arc::clone(&sessions),
                "service/REATTACHED.md",
                "reattached once\n",
                "reattached once\n",
            ))
            .build(&handle)
            .await
            .expect("reattachment world builds");

        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("enqueue"),
            1
        );
        let session_pause = stack.pause_hooks().arm(PausePoint::AgentSessionStarted);
        let first_heartbeat = stack
            .pause_hooks()
            .arm(PausePoint::WorkerHeartbeatReportingJob);
        stack.start_worker(&handle);
        let session = session_pause.arrived().await;
        let heartbeat = first_heartbeat.arrived().await;
        let issue = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .unwrap()
            .unwrap();
        let job_id = parse_metadata_block(&issue.body)
            .unwrap()
            .unwrap()
            .assignment
            .and_then(|assignment| assignment.job_id)
            .expect("assignment job id");
        assert!(!job_id.is_empty());

        stack.replace_daemon(&handle).await;
        let reconnected_heartbeat = stack
            .pause_hooks()
            .arm(PausePoint::WorkerHeartbeatReportingJob);
        let heartbeat_completed = stack
            .pause_hooks()
            .arm(PausePoint::WorkerHeartbeatCompleted);
        heartbeat.release();
        reconnected_heartbeat.arrived().await.release();
        heartbeat_completed.arrived().await.release();
        assert!(
            stack.open_recovery_barrier().await.is_empty(),
            "exact heartbeat reattaches staged job"
        );

        session.release();
        let result = stack
            .await_worker_result(&cx, std::time::Duration::from_secs(20))
            .await
            .expect("result");
        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(
            stack
                .wait_for_pull_request_count(&cx, 1, std::time::Duration::from_secs(10))
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(sessions.load(Ordering::SeqCst), 1, "one agent session ran");
        assert_eq!(stack.persisted_session_count().expect("session count"), 1);
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("quiet scan"),
            0
        );
        stack.crash_worker().await;
    });
}

#[test]
fn repaired_head_recovery_waits_for_exact_ci_after_daemon_replacement() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let sessions = Arc::new(AtomicUsize::new(0));
        let mut stack = HermeticRealStackBuilder::new()
            .issue(HermeticIssueSpec::ready_code(
                "Restart merge-conflict repair",
                "Open and then repair one implementation PR.",
            ))
            .fake_model_script(numbered_write_script(
                Arc::clone(&sessions),
                "service/REPAIR.md",
                "initial implementation\n",
                "merge conflict repaired\n",
            ))
            .build(&handle)
            .await
            .expect("repair world builds");

        let initial = stack
            .run_open_pr_job(&cx, &handle)
            .await
            .expect("initial implementation");
        assert_eq!(initial.job_result.status, ResultStatus::Success);
        stack.crash_worker().await;
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        let mut pull = initial.pull_requests[0].clone();
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

        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        let recovered = stack
            .forge()
            .get_pull_request_by_number(stack.primary_repo_id(), pull.number)
            .await
            .unwrap()
            .unwrap();
        assert!(recovered.labels.iter().any(|label| label == "landing"));
        assert!(
            !recovered
                .labels
                .iter()
                .any(|label| label == "merge-conflict")
        );
        let metadata = parse_metadata_block(&recovered.body).unwrap().unwrap();
        assert_eq!(
            metadata.repaired_head.as_deref(),
            Some(repaired_head.as_str())
        );
        assert!(metadata.assignment.is_none() && metadata.lease.is_none());
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

        pushed.release();
        let _ = stack
            .await_worker_result(&cx, std::time::Duration::from_secs(10))
            .await
            .expect("old result observed");
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
        let final_pull = stack
            .forge()
            .get_pull_request_by_number(stack.primary_repo_id(), pull.number)
            .await
            .unwrap()
            .unwrap();
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

#[test]
fn startup_mechanical_reconciliation_unblocks_dependency_before_dispatch() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let mut stack = HermeticRealStackBuilder::new()
            .issue(
                HermeticIssueSpec::ready_code(
                    "Dependent after offline prerequisite",
                    "Dispatch only after the prerequisite lands.",
                )
                .labels(["blocked", "code"]),
            )
            .fake_model_response(FakeModelResponse::write_file(
                "service/DEPENDENT.md",
                "dependency reconciled before dispatch\n",
                "Implemented the unblocked dependant.",
            ))
            .build(&handle)
            .await
            .expect("dependency world builds");
        let prerequisite = stack
            .forge()
            .create_issue(
                stack.primary_repo_id(),
                CreateIssue {
                    title: "Prerequisite".to_string(),
                    body: "Land while Temper is offline.".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("create prerequisite");
        let dependent = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .unwrap()
            .unwrap();
        stack
            .forge()
            .add_issue_dependency(&dependent.id, prerequisite.number)
            .await
            .expect("wire dependency");

        stack.daemon().crash().await;
        stack
            .forge()
            .update_issue(
                &prerequisite.id,
                UpdateIssue {
                    state: Some(IssueState::Closed),
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("land prerequisite offline");
        stack.replace_daemon(&handle).await;
        let reconciled = stack
            .reconcile_startup_mechanical()
            .await
            .expect("startup reconcile");
        assert_eq!(reconciled.actions, 1);
        let unblocked = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unblocked.labels, vec!["code", "ready"]);
        assert!(
            parse_metadata_block(&unblocked.body)
                .unwrap()
                .unwrap_or_default()
                .assignment
                .is_none()
        );

        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("enqueue behind barrier"),
            1
        );
        let assignment_pause = stack
            .pause_hooks()
            .arm(PausePoint::AssignmentClaimCommitted);
        stack.start_worker(&handle);
        assert!(stack.open_recovery_barrier().await.is_empty());
        assignment_pause.arrived().await.release();
        let result = stack
            .await_worker_result(&cx, std::time::Duration::from_secs(20))
            .await
            .expect("dependent result");
        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(
            stack
                .wait_for_pull_request_count(&cx, 1, std::time::Duration::from_secs(10))
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            stack
                .reconcile_startup_mechanical()
                .await
                .expect("quiet mechanical"),
            temper_runner::Progress::unchanged()
        );
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("quiet role"),
            0
        );
        assert_eq!(stack.persisted_session_count().expect("session count"), 1);
        stack.crash_worker().await;
    });
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
