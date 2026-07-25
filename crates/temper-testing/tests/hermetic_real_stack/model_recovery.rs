// SPDX-License-Identifier: MPL-2.0

//! End-to-end bounded model-recovery acceptance over the native real stack.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use temper_protocol_activity::ModelFailureCategoryV1;
use temper_protocol_worker::{FailureClass, ResultStatus, SessionRecoveryActionV1};
use temper_testing::real_stack::{HermeticIssueSpec, PausePoint};
use temper_worker::AgentSessionStore;

#[path = "model_recovery/support.rs"]
mod support;
use support::*;

#[test]
fn model_recovery_rotates_once_across_daemon_replacement_and_publishes_preserved_product() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let model_calls = Arc::new(AtomicUsize::new(0));
        let mut stack = model_recovery_builder()
            .issue(HermeticIssueSpec::ready_code(
                "Recover a product after one consumed model session",
                "Preserve predecessor edits and complete them from one fresh session.",
            ))
            .fake_model_script(recovery_success_script(Arc::clone(&model_calls)))
            .max_iterations(6)
            .apply_grace(Duration::ZERO)
            .build(&handle)
            .await
            .expect("model-recovery success world builds");

        assert!(
            stack.trace_runs().is_err(),
            "the capstone must not depend on agent activity capture"
        );
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("initial model-recovery work enqueues"),
            1
        );
        stack.start_worker(&handle);

        let rotation = stack
            .await_worker_result(&cx, Duration::from_secs(20))
            .await
            .expect("first non-retryable terminal publishes a rotation result");
        let (prior_session_id, fresh_session_id) = assert_rotation_result(&rotation);
        let rotation_attempt = rotation
            .attempt_id
            .clone()
            .expect("rotation result has an attempt fence");
        let released = wait_for_ready_issue(&stack, &cx).await;
        assert!(!released.labels.iter().any(|label| label == "needs-human"));
        assert_eq!(
            wait_for_accepted_release_count(&stack, &cx, &rotation_attempt).await,
            1,
            "the consumed session releases its claim exactly once"
        );
        assert!(stack.pull_requests().await.unwrap().is_empty());

        let coordination_key = format!("pr-for-code-{}", stack.issue_number().get());
        let work_branch = format!("agent/{coordination_key}");
        let checkout = stack
            .workspace_checkout(stack.primary_repo_path())
            .expect("the consumed session checkout remains available");
        assert_dirty_recovery_work(&checkout);
        let store = AgentSessionStore::for_workspace_root(
            stack.workspace_root(),
            "engineer",
            &coordination_key,
        )
        .expect("coordination-scoped session store");
        let ledger_after_rotation = store
            .load_ledger_sync()
            .expect("rotation ledger loads")
            .expect("rotation ledger exists");
        assert_rotated_ledger(
            &ledger_after_rotation,
            &rotation,
            &prior_session_id,
            &fresh_session_id,
        );

        stack.crash_worker().await;
        stack.replace_daemon(&handle).await;
        assert!(
            stack.open_recovery_barrier().await.is_empty(),
            "the accepted rotation result left no stale assignment"
        );
        let fresh_started = stack.pause_hooks().arm(PausePoint::AgentSessionStarted);
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("the one authorized recovery scan enqueues"),
            1
        );
        stack.start_worker(&handle);
        let fresh_started = fresh_started.arrived().await;
        assert_eq!(
            stack
                .workspace_checkout(stack.primary_repo_path())
                .expect("fresh session checkout"),
            checkout,
            "rotation must reuse the exact coordination-scoped checkout"
        );
        assert_dirty_recovery_work(&checkout);
        assert_eq!(
            observed_session_ids(&stack),
            vec![prior_session_id.clone(), fresh_session_id.clone()],
            "the replacement worker must attach one distinct fresh session"
        );
        assert_eq!(
            store
                .load_sync()
                .expect("active session reload")
                .expect("active session exists")
                .session_id,
            fresh_session_id
        );
        fresh_started.release();

        let success =
            await_result_after_attempt(&cx, &mut stack, &rotation_attempt, Duration::from_secs(20))
                .await
                .expect("fresh session completes the preserved product");
        assert_eq!(success.status, ResultStatus::Success, "{success:#?}");
        assert_eq!(success.repos.len(), 1);
        assert_eq!(success.repos[0].branch.name, work_branch);
        let pulls = stack
            .wait_for_pull_request_count(&cx, 1, Duration::from_secs(10))
            .await
            .expect("one implementation PR appears");
        assert_eq!(pulls.len(), 1);
        assert_eq!(pulls[0].source.branch, work_branch);
        assert!(
            pulls[0]
                .body
                .contains("Recovered predecessor workspace changes.")
        );
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), &work_branch, "README.md")
                .expect("tracked predecessor work is pushed"),
            TRACKED_CONTENT
        );
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), &work_branch, UNTRACKED_PATH)
                .expect("untracked predecessor work is pushed"),
            UNTRACKED_CONTENT
        );
        assert_eq!(
            sorted_branches(&stack),
            vec![work_branch.clone(), "main".to_string()],
            "only one product branch is published"
        );
        assert_eq!(
            stack
                .origin_log_subjects(stack.primary_repo_path(), &work_branch, 10)
                .expect("product history")
                .len(),
            2,
            "the predecessor and fresh session changes form one product commit"
        );

        let publications = stack.published_results();
        assert_eq!(publications.len(), 2);
        assert_eq!(
            publications
                .iter()
                .filter(|result| {
                    result.failure.as_ref().is_some_and(|failure| {
                        failure.class == FailureClass::Transient
                            && failure.session_recovery.as_ref().is_some_and(|recovery| {
                                recovery.action == SessionRecoveryActionV1::RotateSession
                            })
                    })
                })
                .count(),
            1,
            "exactly one transient rotation result is published"
        );
        assert_eq!(
            publications
                .iter()
                .filter(|result| result.status == ResultStatus::Success && result.repos.len() == 1)
                .count(),
            1,
            "exactly one successful product is published"
        );
        let completed_ledger = store
            .load_ledger_sync()
            .expect("completed ledger loads")
            .expect("completed ledger exists");
        assert_eq!(completed_ledger.active_session.session_id, fresh_session_id);
        assert_eq!(
            completed_ledger
                .prior_session
                .as_ref()
                .expect("predecessor evidence remains")
                .session
                .session_id,
            prior_session_id
        );
        assert_eq!(completed_ledger.failure_epoch, 2);
        assert_eq!(completed_ledger.consecutive_terminal_count, 0);
        assert!(!completed_ledger.rotation_consumed);
        assert!(completed_ledger.accounted_attempt_id.is_none());
        assert!(completed_ledger.recovery_decision.is_none());
        assert_eq!(model_calls.load(Ordering::SeqCst), 5);
        assert_no_human_attention(&stack).await;
        stack.crash_worker().await;
    });
}

#[test]
fn model_recovery_exhaustion_parks_once_and_never_reclaims_after_restarts() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let model_calls = Arc::new(AtomicUsize::new(0));
        let mut stack = model_recovery_builder()
            .issue(HermeticIssueSpec::ready_code(
                "Park after the fresh model session also fails",
                "Preserve all dirty work and stop automatic claims after bounded recovery.",
            ))
            .fake_model_script(recovery_exhaustion_script(Arc::clone(&model_calls)))
            .max_iterations(6)
            .apply_grace(Duration::ZERO)
            .build(&handle)
            .await
            .expect("model-recovery exhaustion world builds");

        assert!(
            stack.trace_runs().is_err(),
            "typed recovery must converge while activity capture is unavailable"
        );
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("initial exhausted-recovery work enqueues"),
            1
        );
        stack.start_worker(&handle);
        let rotation = stack
            .await_worker_result(&cx, Duration::from_secs(20))
            .await
            .expect("first failure rotates");
        let rotation_attempt = rotation
            .attempt_id
            .clone()
            .expect("rotation result has an attempt fence");
        let (prior_session_id, fresh_session_id) = assert_rotation_result(&rotation);
        wait_for_ready_issue(&stack, &cx).await;

        let coordination_key = format!("pr-for-code-{}", stack.issue_number().get());
        let store = AgentSessionStore::for_workspace_root(
            stack.workspace_root(),
            "engineer",
            &coordination_key,
        )
        .expect("coordination-scoped session store");
        let checkout = stack
            .workspace_checkout(stack.primary_repo_path())
            .expect("preserved checkout");
        assert_dirty_recovery_work(&checkout);

        stack.crash_worker().await;
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        let fresh_started = stack.pause_hooks().arm(PausePoint::AgentSessionStarted);
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("fresh-session recovery enqueues once"),
            1
        );
        stack.start_worker(&handle);
        let fresh_started = fresh_started.arrived().await;
        assert_eq!(
            stack
                .workspace_checkout(stack.primary_repo_path())
                .expect("fresh failed session checkout"),
            checkout
        );
        assert_dirty_recovery_work(&checkout);
        assert_eq!(
            observed_session_ids(&stack),
            vec![prior_session_id.clone(), fresh_session_id.clone()]
        );
        fresh_started.release();

        let parked_result =
            await_result_after_attempt(&cx, &mut stack, &rotation_attempt, Duration::from_secs(20))
                .await
                .expect("second non-retryable terminal publishes a permanent park");
        assert_park_result(&parked_result, &prior_session_id, &fresh_session_id);
        let parked_issue = wait_for_parked_issue(&stack, &cx).await;
        assert_dirty_recovery_work(&checkout);
        assert!(stack.pull_requests().await.unwrap().is_empty());
        assert_eq!(sorted_branches(&stack), vec!["main".to_string()]);
        assert_eq!(
            comments(&stack, &parked_issue).await.len(),
            1,
            "one model-recovery audit is published"
        );
        assert_actionable_park_audit(&comments(&stack, &parked_issue).await[0].body);

        let exhausted_ledger = store
            .load_ledger_sync()
            .expect("exhausted ledger loads")
            .expect("exhausted ledger exists");
        assert_eq!(exhausted_ledger.active_session.session_id, fresh_session_id);
        assert_eq!(
            exhausted_ledger
                .prior_session
                .as_ref()
                .expect("prior session is archived")
                .session
                .session_id,
            prior_session_id
        );
        assert_eq!(exhausted_ledger.failure_epoch, 1);
        assert_eq!(exhausted_ledger.consecutive_terminal_count, 1);
        assert!(exhausted_ledger.rotation_consumed);
        assert_eq!(
            exhausted_ledger
                .recovery_decision
                .as_ref()
                .expect("park decision is durable")
                .action,
            SessionRecoveryActionV1::ParkForHuman
        );
        assert_eq!(
            exhausted_ledger
                .latest_model_failure
                .as_ref()
                .unwrap()
                .category,
            ModelFailureCategoryV1::Authentication
        );

        redeliver_result(&stack, parked_result.clone()).await;
        for _ in 0..3 {
            assert_eq!(
                stack
                    .enqueue_scanned_role_work(stack.clock().now())
                    .await
                    .expect("parked repeated scan succeeds"),
                0,
                "the parked issue must not be claimed again"
            );
        }
        assert_eq!(
            comments(&stack, &current_issue(&stack).await).await.len(),
            1
        );

        stack.crash_worker().await;
        stack.replace_daemon_through_startup_recovery(&handle).await;
        assert!(
            stack.open_recovery_barrier().await.is_empty(),
            "a parked source has no assignment to recover"
        );
        stack.start_worker(&handle);
        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(100)).await;
        for _ in 0..3 {
            assert_eq!(
                stack
                    .enqueue_scanned_role_work(stack.clock().now())
                    .await
                    .expect("post-restart parked scan succeeds"),
                0
            );
        }
        stack.crash_worker().await;

        let final_issue = current_issue(&stack).await;
        assert_parked_without_assignment(&final_issue);
        assert_eq!(comments(&stack, &final_issue).await.len(), 1);
        assert_dirty_recovery_work(&checkout);
        assert_eq!(
            observed_session_ids(&stack).len(),
            2,
            "no third session runs"
        );
        assert_eq!(model_calls.load(Ordering::SeqCst), 4);
        assert!(stack.pull_requests().await.unwrap().is_empty());
        let publications = stack.published_results();
        assert_eq!(publications.len(), 2, "result replay is attempt-idempotent");
        assert_eq!(
            publications
                .iter()
                .filter(|result| {
                    result.failure.as_ref().is_some_and(|failure| {
                        failure.class == FailureClass::Permanent
                            && failure.session_recovery.as_ref().is_some_and(|recovery| {
                                recovery.action == SessionRecoveryActionV1::ParkForHuman
                            })
                    })
                })
                .count(),
            1,
            "one finite permanent boundary is published"
        );
    });
}
