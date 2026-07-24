use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use temper_engine::{CiStatusTransition, CiTerminalTransition, CiTerminalVerdict};
use temper_forge_memory::FaultOp;
use temper_forge_model::{
    ChangeHint, ChangeKind, CiJob, CiJobConclusion, CiJobStatus, CiRetryOutcome, Forge, ItemNumber,
    PullRequest, PullRequestState, RepositoryPath,
};
use temper_protocol_worker::{JobResult, ResultStatus};
use temper_testing::real_stack::{
    FakeModelResponse, HermeticCiAttempt, HermeticCiJobSpec, HermeticIssueSpec, HermeticRealStack,
    HermeticRealStackBuilder, Reply, Script, StopReason, Turn, WorkerRoleSpec,
};
use temper_workflow::{CiStatus, parse_metadata_block, requires_human_attention};

const RUN_ID: &str = "591";
const RUN_URL: &str = "https://forge.example/acme/service/actions/runs/591";

#[test]
fn runner_loss_restart_retry_pending_then_pass_preserves_the_exact_head() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let mut stack = runner_loss_stack_builder()
            .issue(HermeticIssueSpec::ready_code(
                "Runner-loss pass recovery",
                "Open an implementation PR, then recover its interrupted CI attempt.",
            ))
            .fake_model_response(FakeModelResponse::write_file(
                "service/RECOVERY.md",
                "initial implementation\n",
                "Created the recovery fixture implementation.",
            ))
            .build(&handle)
            .await
            .expect("runner-loss pass world builds");
        let (pull, head, branch) = open_initial_pull(&cx, &handle, &mut stack).await;
        let original_log = stack
            .origin_log_subjects(stack.primary_repo_path(), &branch, 8)
            .expect("initial branch log");

        let running = running_attempt(&stack, &head, "1");
        stack
            .seed_ci_attempt(pull.number, running)
            .await
            .expect("running current-head attempt is visible");
        assert_eq!(
            stack
                .run_ci_status_monitor_cadence()
                .await
                .expect("running cadence"),
            0,
            "a running attempt after tests begin has no fabricated verdict"
        );

        // Execution infrastructure disappears while CI is running. The old
        // daemon never observes a terminal result; the replacement sees the
        // provider's delayed runner-lost terminalization.
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        let terminal_jobs = stack
            .seed_ci_attempt(pull.number, runner_lost_attempt(&stack, &head, "1"))
            .await
            .expect("delayed terminalization is retained");
        stack.forge().set_ci_retry_outcome(CiRetryOutcome::Accepted);
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .expect("fresh recovery scan"),
            0
        );
        assert_one_exact_retry(&stack, &pull, &head, "1");
        assert_eq!(stack.persisted_session_count().unwrap(), 1);
        assert_unchanged_head_and_history(&stack, &pull, &head, &branch, &original_log).await;

        // Daemon replacement at the durable retry boundary and duplicate scans
        // cannot issue the provider request again.
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .expect("duplicate terminal scan"),
            0
        );
        assert_one_exact_retry(&stack, &pull, &head, "1");

        let pending = pending_attempt(&stack, &head, "2");
        stack
            .seed_ci_attempt(pull.number, pending)
            .await
            .expect("newer retry attempt is pending");

        // Duplicate delayed terminal hints are coalesced wake-ups. The
        // production wake executor re-reads attempt 2, clears the stale marker,
        // and never dispatches repair from the old runner-loss fact.
        submit_terminal_hint(&stack, pull.number, &head, &terminal_jobs);
        submit_terminal_hint(&stack, pull.number, &head, &terminal_jobs);
        wait_for_recovery_marker(&cx, &stack, pull.number, false).await;
        assert_one_exact_retry(&stack, &pull, &head, "1");
        assert_eq!(stack.persisted_session_count().unwrap(), 1);
        assert_unchanged_head_and_history(&stack, &pull, &head, &branch, &original_log).await;
        assert_eq!(
            stack
                .reconcile_targeted_ci_mechanical(pull.number)
                .await
                .expect("pending retry cannot land"),
            temper_runner::Progress::unchanged()
        );

        stack
            .seed_ci_attempt(pull.number, successful_attempt(&stack, &head, "2"))
            .await
            .expect("retry attempt passes authoritatively");
        assert!(
            stack
                .reconcile_targeted_ci_mechanical(pull.number)
                .await
                .expect("green retry lands")
                .changed
        );
        let landed = current_pull(&stack, pull.number).await;
        assert_eq!(landed.state, PullRequestState::Merged);
        assert_eq!(landed.head_sha.as_deref(), Some(head.as_str()));
        assert_eq!(stack.persisted_session_count().unwrap(), 1);
        assert_eq!(
            stack
                .origin_log_subjects(stack.primary_repo_path(), &branch, 8)
                .unwrap(),
            original_log,
            "interruption recovery must not create a synthetic commit"
        );
    });
}

#[test]
fn changed_head_during_runner_loss_recovery_suppresses_the_stale_attempt() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let mut stack = runner_loss_stack_builder()
            .issue(HermeticIssueSpec::ready_code(
                "Runner-loss exact-head ownership",
                "Suppress interruption recovery if another actor advances the PR head.",
            ))
            .fake_model_response(FakeModelResponse::write_file(
                "service/RECOVERY.md",
                "initial implementation\n",
                "Created the exact-head fixture implementation.",
            ))
            .build(&handle)
            .await
            .expect("head-change world builds");
        let (pull, head, branch) = open_initial_pull(&cx, &handle, &mut stack).await;
        let terminal_jobs = stack
            .seed_ci_attempt(pull.number, runner_lost_attempt(&stack, &head, "1"))
            .await
            .unwrap();
        stack.forge().set_ci_retry_outcome(CiRetryOutcome::Accepted);
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .unwrap(),
            0
        );
        assert_one_exact_retry(&stack, &pull, &head, "1");
        assert!(recovery_state(&stack, pull.number).await.is_some());

        // This is an explicitly simulated external push, not an interruption
        // recovery side effect. A reordered old-head terminal hint must re-read
        // the new head and relinquish the stale marker without retry, repair, or
        // parking actions against either head.
        let changed_head = stack
            .advance_origin_branch(
                stack.primary_repo_path(),
                &branch,
                "EXTERNAL_HEAD.txt",
                "advanced by another actor\n",
            )
            .expect("external actor advances PR branch");
        stack
            .forge()
            .set_pull_request_head(&pull.id, Some(changed_head.clone()))
            .unwrap();
        stack
            .publish_pull_request_head_ref(pull.number, &changed_head)
            .unwrap();
        submit_terminal_hint(&stack, pull.number, &head, &terminal_jobs);
        wait_for_recovery_marker(&cx, &stack, pull.number, false).await;

        assert_one_exact_retry(&stack, &pull, &head, "1");
        assert_eq!(stack.persisted_session_count().unwrap(), 1);
        let changed = current_pull(&stack, pull.number).await;
        assert_eq!(changed.head_sha.as_deref(), Some(changed_head.as_str()));
        assert!(!requires_human_attention(&changed.labels));
        assert!(
            stack
                .forge()
                .list_pull_request_comments(&pull.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("new pending/missing head is not a source repair"),
            0
        );
        assert_eq!(
            stack
                .origin_rev(stack.primary_repo_path(), &branch)
                .unwrap(),
            changed_head
        );
    });
}

#[test]
fn runner_loss_retry_then_ordinary_failure_enters_only_the_writable_repair_route() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let sessions = Arc::new(AtomicUsize::new(0));
        let mut stack = runner_loss_stack_builder()
            .issue(HermeticIssueSpec::ready_code(
                "Runner-loss then ordinary failure",
                "Repair only after a newer retry reports an ordinary failure.",
            ))
            .fake_model_script(numbered_repair_script(Arc::clone(&sessions)))
            .build(&handle)
            .await
            .expect("ordinary failure world builds");
        let (pull, head, branch) = open_initial_pull(&cx, &handle, &mut stack).await;
        let original_log = stack
            .origin_log_subjects(stack.primary_repo_path(), &branch, 8)
            .unwrap();

        stack
            .seed_ci_attempt(pull.number, runner_lost_attempt(&stack, &head, "1"))
            .await
            .unwrap();
        stack.forge().set_ci_retry_outcome(CiRetryOutcome::Accepted);
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .unwrap(),
            0
        );
        assert_one_exact_retry(&stack, &pull, &head, "1");
        stack
            .seed_ci_attempt(pull.number, pending_attempt(&stack, &head, "2"))
            .await
            .unwrap();
        let old_jobs = ci_jobs_for_attempt(&stack, "1").await;
        submit_terminal_hint(&stack, pull.number, &head, &old_jobs);
        wait_for_recovery_marker(&cx, &stack, pull.number, false).await;

        assert_eq!(sessions.load(Ordering::SeqCst), 1);
        assert_unchanged_head_and_history(&stack, &pull, &head, &branch, &original_log).await;
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("pending retry has no engineer work"),
            0
        );

        stack
            .seed_ci_attempt(pull.number, failed_attempt(&stack, &head, "2"))
            .await
            .unwrap();
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("ordinary failure queues repair"),
            1
        );
        stack.start_worker(&handle);
        let repaired = await_result_for_role(&cx, &mut stack, "/engineer/pr_ci_failed").await;
        assert_eq!(
            repaired.status,
            ResultStatus::Success,
            "repair failed: {:?}",
            repaired.failure
        );
        assert!(
            !repaired.repos.is_empty(),
            "repair produced no repository result: {repaired:?}; sessions={}",
            sessions.load(Ordering::SeqCst)
        );
        let repaired_head = repaired.repos[0].branch.head_sha.clone();
        assert_eq!(
            stack
                .origin_rev(stack.primary_repo_path(), &branch)
                .expect("repair head pushed"),
            repaired_head
        );
        assert_ne!(repaired_head, head);
        assert_eq!(sessions.load(Ordering::SeqCst), 2);
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), &branch, "RECOVERY.md")
                .unwrap(),
            "ordinary failure repaired\n"
        );
        assert_eq!(stack.forge().ci_retry_requests().len(), 1);
        stack.crash_worker().await;
    });
}

#[test]
fn unsupported_retry_restarts_at_diagnostic_parking_audit_and_cleanup_boundaries() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let sessions = Arc::new(AtomicUsize::new(0));
        let mut stack = diagnostic_stack(Arc::clone(&sessions))
            .build(&handle)
            .await
            .expect("unsupported recovery world builds");
        let (pull, head, branch) = open_initial_pull(&cx, &handle, &mut stack).await;
        let original_log = stack
            .origin_log_subjects(stack.primary_repo_path(), &branch, 8)
            .unwrap();
        stack
            .seed_ci_attempt(pull.number, runner_lost_attempt(&stack, &head, "1"))
            .await
            .unwrap();

        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("engineer", stack.clock().now())
                .await
                .expect("runner loss never queues writable repair"),
            0
        );
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .expect("unsupported retry is recorded"),
            0
        );
        assert_one_exact_retry(&stack, &pull, &head, "1");
        let recovery = recovery_state(&stack, pull.number).await.unwrap();
        assert_eq!(recovery.retry_outcome, Some(CiRetryOutcome::Unsupported));

        // Retry marker survives daemon replacement. The diagnostic claim and
        // result are durable before a second replacement, which must observe the
        // exhausted publication boundary rather than dispatch it again.
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .expect("diagnostic queues"),
            1
        );
        stack.start_worker(&handle);
        let diagnostic =
            await_result_for_role(&cx, &mut stack, "/ci_diagnostician/pr_ci_recovery").await;
        assert_eq!(diagnostic.status, ResultStatus::Success);
        assert_eq!(diagnostic.verdict.as_deref(), Some("diagnosed"));
        wait_for_assignment_clear(&cx, &stack, pull.number).await;
        stack.crash_worker().await;
        assert_eq!(sessions.load(Ordering::SeqCst), 2);
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        assert_unchanged_head_and_history(&stack, &pull, &head, &branch, &original_log).await;

        // Crash after installing attention but before audit publication. This
        // proves the durable attention barrier survives component replacement.
        stack.forge().fail_next(
            FaultOp::AddPullRequestComment,
            "lose daemon after interrupted-CI attention barrier",
        );
        assert!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .is_err()
        );
        let attention = current_pull(&stack, pull.number).await;
        assert!(requires_human_attention(&attention.labels));
        assert!(
            recovery_state(&stack, pull.number)
                .await
                .unwrap()
                .parking_barrier_installed
        );
        assert!(
            stack
                .forge()
                .list_pull_request_comments(&pull.id)
                .await
                .unwrap()
                .is_empty()
        );

        // Replacement publishes the audit, then loses the cleanup CAS. The
        // next replacement reuses the fingerprint-keyed comment and clears only
        // the transient marker.
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        stack.forge().conflict_next(
            FaultOp::UpdatePullRequest,
            "lose daemon after interrupted-CI audit publication",
        );
        assert!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .is_err()
        );
        assert_actionable_single_audit(&stack, &pull, &head).await;
        assert!(recovery_state(&stack, pull.number).await.is_some());

        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .expect("cleanup resumes"),
            0
        );
        assert!(recovery_state(&stack, pull.number).await.is_none());
        assert_actionable_single_audit(&stack, &pull, &head).await;
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        assert_actionable_single_audit(&stack, &pull, &head).await;
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("engineer", stack.clock().now())
                .await
                .expect("parked PR is not repairable"),
            0
        );
        assert_unchanged_head_and_history(&stack, &pull, &head, &branch, &original_log).await;
    });
}

#[test]
fn accepted_retry_exhaustion_runs_one_diagnostic_then_parks_once() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let sessions = Arc::new(AtomicUsize::new(0));
        let mut stack = diagnostic_stack(Arc::clone(&sessions))
            .build(&handle)
            .await
            .expect("exhausted retry world builds");
        let (pull, head, branch) = open_initial_pull(&cx, &handle, &mut stack).await;
        let original_log = stack
            .origin_log_subjects(stack.primary_repo_path(), &branch, 8)
            .unwrap();
        stack
            .seed_ci_attempt(pull.number, runner_lost_attempt(&stack, &head, "1"))
            .await
            .unwrap();
        stack.forge().set_ci_retry_outcome(CiRetryOutcome::Accepted);
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .unwrap(),
            0
        );
        assert_one_exact_retry(&stack, &pull, &head, "1");

        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        stack.clock().advance(chrono::Duration::minutes(6));
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .expect("exhausted retry dispatches fallback"),
            1
        );
        stack.start_worker(&handle);
        let diagnostic =
            await_result_for_role(&cx, &mut stack, "/ci_diagnostician/pr_ci_recovery").await;
        assert_eq!(
            diagnostic.verdict.as_deref(),
            Some("diagnosed"),
            "unexpected diagnostic result: {diagnostic:?}; sessions={}",
            sessions.load(Ordering::SeqCst)
        );
        wait_for_assignment_clear(&cx, &stack, pull.number).await;
        stack.crash_worker().await;
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("ci_diagnostician", stack.clock().now())
                .await
                .expect("diagnostic exhaustion parks"),
            0
        );
        let after_exhausted_diagnostic = current_pull(&stack, pull.number).await;
        assert!(
            requires_human_attention(&after_exhausted_diagnostic.labels),
            "exhausted diagnostic did not park: labels={:?} metadata={:?}",
            after_exhausted_diagnostic.labels,
            parse_metadata_block(&after_exhausted_diagnostic.body).unwrap()
        );
        assert!(recovery_state(&stack, pull.number).await.is_none());
        assert_actionable_single_audit(&stack, &pull, &head).await;
        assert_one_exact_retry(&stack, &pull, &head, "1");
        assert_eq!(sessions.load(Ordering::SeqCst), 2);
        assert_unchanged_head_and_history(&stack, &pull, &head, &branch, &original_log).await;
    });
}

include!("hermetic_runner_loss_restart/support.rs");
