// SPDX-License-Identifier: MPL-2.0

//! Recovered-assignment ownership-loss acceptance matrix over the real stack.

use std::time::Duration;

use temper_forge_memory::FaultOp;
use temper_protocol_activity::{AgentActivityEventV1, RunFinishedV1, RunStatusV1};
use temper_protocol_worker::{FailureClass, ReleaseDisposition, ResultStatus};
#[cfg(target_os = "linux")]
use temper_testing::real_stack::HermeticRealStackBuilder;
use temper_testing::real_stack::{PausePoint, ReachedPause};
use temper_workflow::{DurableAssignment, parse_metadata_block};

#[path = "ownership_loss_support.rs"]
mod support;
use support::*;

#[derive(Clone, Copy, Debug)]
enum OwnershipLoss {
    Blocked,
    Closed,
    Replaced,
}

impl OwnershipLoss {
    fn expected_release(self) -> ReleaseDisposition {
        match self {
            Self::Blocked | Self::Closed => ReleaseDisposition::Reclaimed,
            Self::Replaced => ReleaseDisposition::Superseded,
        }
    }
}

struct RecoveredAttempt {
    session: ReachedPause,
    reattached_heartbeat: ReachedPause,
    assignment: DurableAssignment,
}

#[cfg(target_os = "linux")]
#[test]
fn ownership_loss_missing_supervisor_helper_fails_before_world_setup() {
    temper_engine_io::block_on_with(|_cx, handle| async move {
        let temporary = tempfile::tempdir().expect("missing-helper tempdir");
        let missing = temporary.path().join("not-built-supervisor-helper");
        let error = match HermeticRealStackBuilder::new()
            .linux_supervisor_helper(&missing)
            .build(&handle)
            .await
        {
            Ok(_) => panic!("a missing required supervisor helper must fail fixture setup"),
            Err(error) => error,
        };
        assert!(error.contains(&missing.display().to_string()), "{error}");
        assert!(
            error.contains("CARGO_BIN_EXE_temper-real-stack-supervisor-helper"),
            "the diagnostic should name the self-contained Cargo target: {error}"
        );
    });
}

#[test]
fn ownership_loss_blocked_issue_cancels_recovered_attempt_and_compacts_stale_result() {
    run_ownership_loss(OwnershipLoss::Blocked);
}

#[test]
fn ownership_loss_closed_issue_cancels_recovered_attempt_and_compacts_stale_result() {
    run_ownership_loss(OwnershipLoss::Closed);
}

#[test]
fn ownership_loss_newer_attempt_cancels_only_replaced_recovered_attempt() {
    run_ownership_loss(OwnershipLoss::Replaced);
}

fn run_ownership_loss(loss: OwnershipLoss) {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let mut stack = ownership_world(&handle).await;
        let recovered = start_recovered_attempt(&mut stack, &cx, &handle).await;
        let job_id = recovered
            .assignment
            .job_id
            .clone()
            .expect("assignment job id");
        let attempt_id = recovered
            .assignment
            .attempt_id
            .clone()
            .expect("assignment attempt id");
        let correlation_key = recovered
            .assignment
            .coordination_key
            .clone()
            .expect("assignment coordination key");

        let ownership_heartbeat = stack
            .pause_hooks()
            .arm(PausePoint::WorkerHeartbeatReportingJob);
        recovered.reattached_heartbeat.release();
        let ownership_heartbeat =
            await_pause(&cx, ownership_heartbeat, "ownership-check heartbeat").await;
        let replacement_attempt = mutate_durable_ownership(&stack, loss).await;

        let cancel_requested = stack.pause_hooks().arm(PausePoint::WorkerCancelRequested);
        let terminal_acknowledgement = stack
            .pause_hooks()
            .arm(PausePoint::WorkerTerminalTraceAcknowledgement);
        let quiesced = stack.pause_hooks().arm(PausePoint::WorkerQuiesced);
        let result_recorded = stack.pause_hooks().arm(PausePoint::WorkerResultRecorded);
        let delivery_resolved = stack
            .pause_hooks()
            .arm(PausePoint::WorkerResultDeliveryResolved);
        let result_acknowledged = stack
            .pause_hooks()
            .arm(PausePoint::WorkerResultAcknowledged);
        let capacity_released = stack.pause_hooks().arm(PausePoint::WorkerCapacityReleased);
        ownership_heartbeat.release();

        let cancel_requested =
            await_pause(&cx, cancel_requested, "ownership-loss fence closure").await;
        let active = stack.active_worker_tasks();
        assert_eq!(
            active.len(),
            1,
            "the exact local attempt still owns its slot"
        );
        assert_eq!(active[0].job_id(), job_id);
        assert_eq!(active[0].attempt_id(), attempt_id);
        assert!(
            !active[0].fence().is_open(),
            "the exact attempt fence closes"
        );
        assert!(
            stack
                .daemon()
                .workstream_active_by_correlation_key(&correlation_key)
                .await,
            "the daemon slot remains occupied while cancellation is joining"
        );
        assert_eq!(stack.pending_result_count().unwrap(), 0);
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerCapacityReleased),
            0,
            "capacity cannot release at fence closure"
        );
        let effects_at_fence = stack.agent_activity_snapshot();
        assert_no_agent_effects(effects_at_fence);
        assert_no_product_effects(&stack).await;

        cancel_requested.release();
        recovered.session.release();
        let terminal_acknowledgement = await_pause(
            &cx,
            terminal_acknowledgement,
            "engine-persisted terminal trace before worker acknowledgement",
        )
        .await;
        let terminal_sequence = assert_cancelled_journal(&stack);
        let local = stack
            .local_trace_runs()
            .expect("local terminal trace spool");
        assert_eq!(local.len(), 1);
        assert_eq!(
            local[0].events.last().expect("local terminal event").seq,
            terminal_sequence
        );
        assert!(
            local[0].acknowledged_seq < terminal_sequence,
            "the transport pause must withhold the terminal acknowledgement"
        );

        // Elapsed time is never quiescence proof. Keep the daemon's terminal
        // acknowledgement blocked beyond the historical 250 ms flush window
        // and prove every worker/daemon ownership surface remains occupied.
        let heartbeat_while_pending = stack
            .pause_hooks()
            .arm(PausePoint::WorkerHeartbeatReportingJob);
        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(300)).await;
        let heartbeat_while_pending = await_pause(
            &cx,
            heartbeat_while_pending,
            "heartbeat membership while terminal acknowledgement is pending",
        )
        .await;
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerQuiesced),
            0,
            "AttemptQuiesced cannot precede terminal trace acknowledgement"
        );
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerResultRecorded),
            0,
            "the canceled result cannot become durable while tracing is pending"
        );
        assert_eq!(stack.pending_result_count().unwrap(), 0);
        assert!(stack.published_results().is_empty());
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerCapacityReleased),
            0
        );
        let active = stack.active_worker_tasks();
        assert_eq!(active.len(), 1, "the registry entry remains occupied");
        assert_eq!(active[0].job_id(), job_id);
        assert_eq!(active[0].attempt_id(), attempt_id);
        assert!(!active[0].fence().is_open());
        assert!(
            stack
                .daemon()
                .workstream_active_by_correlation_key(&correlation_key)
                .await,
            "daemon capacity remains occupied until trace acknowledgement"
        );
        heartbeat_while_pending.release();
        terminal_acknowledgement.release();

        let quiesced = await_pause(
            &cx,
            quiesced,
            "recursive cleanup, endpoint joins, and terminal trace acknowledgement",
        )
        .await;
        assert_eq!(stack.pending_result_count().unwrap(), 0);
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerCapacityReleased),
            0,
            "cleanup alone is not the durable cancellation boundary"
        );
        assert!(
            stack
                .daemon()
                .workstream_active_by_correlation_key(&correlation_key)
                .await
        );
        quiesced.release();

        let result_recorded = await_pause(&cx, result_recorded, "durable canceled result").await;
        wait_for_outbox_count(&stack, &cx, 1).await;
        let capacity_released =
            await_pause(&cx, capacity_released, "post-durability capacity release").await;
        capacity_released.release();
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerCapacityReleased),
            1,
            "the local permit releases exactly once after durability"
        );
        result_recorded.release();

        let delivery_resolved =
            await_pause(&cx, delivery_resolved, "stale canceled-result release").await;
        let publications = stack.published_results();
        assert_eq!(
            publications.len(),
            1,
            "the canceled result is the sole publication"
        );
        let canceled = &publications[0];
        assert_eq!(canceled.job_id, job_id);
        assert_eq!(canceled.attempt_id.as_deref(), Some(attempt_id.as_str()));
        assert_eq!(canceled.status, ResultStatus::Failure);
        assert_eq!(
            canceled.failure.as_ref().map(|failure| failure.class),
            Some(FailureClass::Canceled)
        );
        assert!(canceled.repos.is_empty());
        let cleanup =
            &canceled.details.as_ref().expect("cleanup evidence")["cancellation"]["cleanup"];
        assert!(cleanup["recursive_empty"].as_bool().unwrap_or(false));
        assert!(cleanup["quiesced"].as_bool().unwrap_or(false));
        assert!(
            cleanup["resources"]
                .as_object()
                .expect("resource join evidence")
                .values()
                .all(|resource| matches!(
                    resource["status"].as_str(),
                    Some("joined" | "not_applicable")
                )),
            "every endpoint/resource join is terminal: {cleanup}"
        );

        let releases = stack.published_releases();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].disposition, loss.expected_release());
        assert_eq!(releases[0].job_id, job_id);
        assert_eq!(releases[0].attempt_id.as_deref(), Some(attempt_id.as_str()));
        assert!(
            !stack
                .daemon()
                .workstream_active_by_correlation_key(&correlation_key)
                .await,
            "stale terminal delivery releases the daemon slot"
        );
        assert_eq!(stack.pending_result_count().unwrap(), 1);
        assert_eq!(stack.agent_activity_snapshot(), effects_at_fence);
        assert_no_product_effects(&stack).await;
        assert_cancelled_trace(&stack, &cx).await;

        if let Some(replacement_attempt) = replacement_attempt {
            let issue = current_issue(&stack).await;
            let metadata = parse_metadata_block(&issue.body)
                .unwrap()
                .expect("replacement metadata remains");
            let replacement = metadata.assignment.expect("newer assignment remains");
            assert_eq!(replacement.job_id.as_deref(), Some(job_id.as_str()));
            assert_eq!(
                replacement.attempt_id.as_deref(),
                Some(replacement_attempt.as_str())
            );
            assert!(
                metadata.lease.is_some(),
                "old stale result cannot clear the new lease"
            );
        }

        delivery_resolved.release();
        let result_acknowledged =
            await_pause(&cx, result_acknowledged, "stale release acknowledgement").await;
        assert_eq!(stack.pending_result_count().unwrap(), 1);
        result_acknowledged.release();
        wait_for_outbox_count(&stack, &cx, 0).await;
        assert_eq!(stack.agent_activity_snapshot(), effects_at_fence);
        assert_eq!(stack.published_results().len(), 1);
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerCapacityReleased),
            1
        );

        if !matches!(loss, OwnershipLoss::Replaced) {
            stack.replace_daemon(&handle).await;
            assert!(
                stack.open_recovery_barrier().await.is_empty(),
                "removed ownership is not staged after restart"
            );
            assert_eq!(stack.observed_agent_sessions().len(), 1);
        }
        stack.crash_worker().await;
    });
}

#[test]
fn ownership_loss_transient_backend_failure_reattaches_and_completes_normally() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let mut stack = ownership_world(&handle).await;
        let recovered = start_recovered_attempt(&mut stack, &cx, &handle).await;
        let job_id = recovered.assignment.job_id.clone().expect("job id");
        let attempt_id = recovered.assignment.attempt_id.clone().expect("attempt id");

        let transient_heartbeat = stack
            .pause_hooks()
            .arm(PausePoint::WorkerHeartbeatReportingJob);
        recovered.reattached_heartbeat.release();
        let transient_heartbeat =
            await_pause(&cx, transient_heartbeat, "transient ownership heartbeat").await;
        stack.forge().fail_next(
            FaultOp::GetIssueByNumber,
            "one hermetic recovered-ownership lookup failure",
        );
        let transient_completed = stack
            .pause_hooks()
            .arm(PausePoint::WorkerHeartbeatCompleted);
        transient_heartbeat.release();
        let transient_completed =
            await_pause(&cx, transient_completed, "transient heartbeat response").await;
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerCancelRequested),
            0,
            "one backend failure must not revoke the attempt"
        );
        let active = stack.active_worker_tasks();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].job_id(), job_id);
        assert_eq!(active[0].attempt_id(), attempt_id);
        assert!(active[0].fence().is_open());

        let owned_heartbeat = stack
            .pause_hooks()
            .arm(PausePoint::WorkerHeartbeatReportingJob);
        transient_completed.release();
        let owned_heartbeat =
            await_pause(&cx, owned_heartbeat, "reattachment retry heartbeat").await;
        let owned_completed = stack
            .pause_hooks()
            .arm(PausePoint::WorkerHeartbeatCompleted);
        owned_heartbeat.release();
        let owned_completed =
            await_pause(&cx, owned_completed, "successful reattachment retry").await;
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerCancelRequested),
            0
        );
        owned_completed.release();
        recovered.session.release();

        let result = stack
            .await_worker_result(&cx, Duration::from_secs(20))
            .await
            .expect("reattached attempt completes");
        assert_eq!(result.job_id, job_id);
        assert_eq!(result.attempt_id.as_deref(), Some(attempt_id.as_str()));
        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(
            stack
                .wait_for_pull_request_count(&cx, 1, Duration::from_secs(10))
                .await
                .expect("normal completion opens one PR")
                .len(),
            1
        );
        assert!(stack.published_results().iter().all(|result| {
            result.failure.as_ref().map(|failure| failure.class) != Some(FailureClass::Canceled)
        }));
        wait_for_outbox_count(&stack, &cx, 0).await;
        stack.crash_worker().await;
    });
}

#[test]
fn ownership_loss_restart_does_not_stage_removed_attempt_and_cancels_old_heartbeat_as_unknown() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let mut stack = ownership_world(&handle).await;
        let recovered = start_recovered_attempt(&mut stack, &cx, &handle).await;
        let job_id = recovered.assignment.job_id.clone().expect("job id");
        let attempt_id = recovered.assignment.attempt_id.clone().expect("attempt id");

        let old_heartbeat = stack
            .pause_hooks()
            .arm(PausePoint::WorkerHeartbeatReportingJob);
        recovered.reattached_heartbeat.release();
        let old_heartbeat = await_pause(&cx, old_heartbeat, "old live heartbeat").await;
        mutate_durable_ownership(&stack, OwnershipLoss::Blocked).await;
        stack.replace_daemon(&handle).await;
        assert!(
            stack.open_recovery_barrier().await.is_empty(),
            "the removed assignment is not staged by the restarted daemon"
        );

        let cancel_requested = stack.pause_hooks().arm(PausePoint::WorkerCancelRequested);
        let terminal_forwarding = stack
            .pause_hooks()
            .arm(PausePoint::WorkerTerminalTraceForwarding);
        let terminal_acknowledgement = stack
            .pause_hooks()
            .arm(PausePoint::WorkerTerminalTraceAcknowledgement);
        let acknowledged = stack
            .pause_hooks()
            .arm(PausePoint::WorkerResultAcknowledged);
        old_heartbeat.release();
        let cancel_requested =
            await_pause(&cx, cancel_requested, "unknown old heartbeat cancellation").await;
        let active = stack.active_worker_tasks();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].job_id(), job_id);
        assert_eq!(active[0].attempt_id(), attempt_id);
        assert!(!active[0].fence().is_open());
        cancel_requested.release();
        recovered.session.release();

        let terminal_forwarding = await_pause(
            &cx,
            terminal_forwarding,
            "locally durable terminal trace awaiting daemon forwarding",
        )
        .await;
        let local = stack.local_trace_runs().expect("pending local trace");
        let terminal_sequence = local[0].events.last().expect("terminal trace").seq;
        assert!(matches!(
            &local[0].events.last().expect("terminal trace").event,
            AgentActivityEventV1::RunFinished(RunFinishedV1 {
                status: RunStatusV1::Cancelled,
                ..
            })
        ));
        assert!(local[0].acknowledged_seq < terminal_sequence);
        assert_eq!(stack.pending_result_count().unwrap(), 0);
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerQuiesced),
            0
        );

        // The forwarding future resolves the daemon on every retry/send. Hold
        // the durable terminal before resolution, replace the daemon, then let
        // the existing forwarder deliver to the restarted journal.
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        terminal_forwarding.release();
        let terminal_acknowledgement = await_pause(
            &cx,
            terminal_acknowledgement,
            "restarted daemon terminal acknowledgement",
        )
        .await;
        assert_eq!(assert_cancelled_journal(&stack), terminal_sequence);
        assert_eq!(stack.pending_result_count().unwrap(), 0);
        assert!(stack.published_results().is_empty());
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerCapacityReleased),
            0
        );
        assert_eq!(stack.active_worker_tasks().len(), 1);
        terminal_acknowledgement.release();

        let acknowledged =
            await_pause(&cx, acknowledged, "restarted stale result acknowledgement").await;
        let publications = stack.published_results();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0]
                .failure
                .as_ref()
                .map(|failure| failure.class),
            Some(FailureClass::Canceled)
        );
        assert_eq!(
            stack.published_releases()[0].disposition,
            ReleaseDisposition::Reclaimed
        );
        assert_eq!(stack.pending_result_count().unwrap(), 1);
        acknowledged.release();
        wait_for_outbox_count(&stack, &cx, 0).await;
        assert_cancelled_trace(&stack, &cx).await;

        stack.crash_worker().await;
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        stack.start_worker(&handle);
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("blocked source remains quiet"),
            0
        );
        assert!(stack.active_worker_tasks().is_empty());
        assert_eq!(stack.observed_agent_sessions().len(), 1);
        stack.crash_worker().await;
    });
}
