// SPDX-License-Identifier: MPL-2.0

//! Recovered-assignment ownership-loss acceptance matrix over the real stack.

use std::process::Command;
use std::time::{Duration, Instant};

use temper_engine::AgentTraceRunStatus;
use temper_forge_memory::FaultOp;
use temper_forge_model::{Forge, Issue, IssueState, UpdateIssue};
use temper_protocol_activity::{AgentActivityEventV1, RunFinishedV1, RunStatusV1};
use temper_protocol_worker::{FailureClass, ReleaseDisposition, ResultStatus};
use temper_testing::real_stack::{
    FakeModelResponse, HermeticActivitySnapshot, HermeticIssueSpec, HermeticRealStack,
    HermeticRealStackBuilder, PausePermit, PausePoint, ReachedPause,
};
use temper_workflow::{DurableAssignment, parse_metadata_block, replace_metadata_block};

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
        let quiesced = await_pause(&cx, quiesced, "recursive cleanup and endpoint joins").await;
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

async fn ownership_world(handle: &skein::runtime::RuntimeHandle) -> HermeticRealStack {
    HermeticRealStackBuilder::new()
        .issue(HermeticIssueSpec::ready_code(
            "Recovered ownership acceptance",
            "The recovered attempt must stop if its exact durable authority disappears.\n\n<!-- temper:workflow\n{\"kind\":\"code\"}\n-->",
        ))
        .fake_model_response(FakeModelResponse::write_file(
            "service/OWNED.md",
            "completed while still owned\n",
            "Completed the still-owned recovered attempt.",
        ))
        .enable_agent_traces()
        .worker_heartbeat_interval(Duration::from_millis(100))
        .build(handle)
        .await
        .expect("ownership-loss world builds")
}

async fn start_recovered_attempt(
    stack: &mut HermeticRealStack,
    cx: &skein::cx::Cx,
    handle: &skein::runtime::RuntimeHandle,
) -> RecoveredAttempt {
    assert_eq!(
        stack
            .enqueue_scanned_role_work(stack.clock().now())
            .await
            .expect("source enqueues"),
        1
    );
    let session = stack.pause_hooks().arm(PausePoint::AgentSessionStarted);
    let first_heartbeat = stack
        .pause_hooks()
        .arm(PausePoint::WorkerHeartbeatReportingJob);
    stack.start_worker(handle);
    let first_heartbeat = await_pause(cx, first_heartbeat, "first live heartbeat").await;
    let session = skein::time::timeout(
        temper_engine_io::runtime::timer_now(cx),
        Duration::from_secs(20),
        Box::pin(session.arrived()),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out waiting for active native-agent session; observed_sessions={} active_tasks={:?} publications={:?} outbox={:?}",
            stack.observed_agent_sessions().len(),
            stack
                .active_worker_tasks()
                .iter()
                .map(|task| (task.job_id(), task.join_state()))
                .collect::<Vec<_>>(),
            stack.published_results(),
            stack.pending_result_count()
        )
    });
    let issue = current_issue(stack).await;
    let assignment = parse_metadata_block(&issue.body)
        .unwrap()
        .expect("claimed metadata")
        .assignment
        .expect("durable assignment");

    stack.replace_daemon(handle).await;
    let heartbeat_completed = stack
        .pause_hooks()
        .arm(PausePoint::WorkerHeartbeatCompleted);
    first_heartbeat.release();
    let heartbeat_completed =
        await_pause(cx, heartbeat_completed, "recovered heartbeat reattachment").await;
    assert!(
        stack.open_recovery_barrier().await.is_empty(),
        "the exact attempt reattaches before recovery opens"
    );
    assert_eq!(stack.observed_agent_sessions().len(), 1);
    RecoveredAttempt {
        session,
        reattached_heartbeat: heartbeat_completed,
        assignment,
    }
}

async fn mutate_durable_ownership(
    stack: &HermeticRealStack,
    loss: OwnershipLoss,
) -> Option<String> {
    let issue = current_issue(stack).await;
    let mut metadata = parse_metadata_block(&issue.body)
        .unwrap()
        .expect("claimed metadata");
    let mut update = UpdateIssue {
        expected_version: Some(issue.version),
        remove_assignees: issue.assignees.clone(),
        ..UpdateIssue::default()
    };
    let replacement_attempt = match loss {
        OwnershipLoss::Blocked => {
            metadata.assignment = None;
            metadata.lease = None;
            update.set_labels = Some(vec!["blocked".to_string(), "code".to_string()]);
            None
        }
        OwnershipLoss::Closed => {
            metadata.assignment = None;
            metadata.lease = None;
            update.state = Some(IssueState::Closed);
            update.set_labels = Some(vec!["code".to_string()]);
            None
        }
        OwnershipLoss::Replaced => {
            let assignment = metadata.assignment.as_mut().expect("old assignment");
            let old_attempt = assignment.attempt_id.as_deref().expect("old attempt");
            let replacement = format!("{old_attempt}-newer");
            assignment.attempt_id = Some(replacement.clone());
            Some(replacement)
        }
    };
    update.body = Some(replace_metadata_block(&issue.body, &metadata).unwrap());
    stack
        .forge()
        .update_issue(&issue.id, update)
        .await
        .expect("durable ownership mutation lands");
    replacement_attempt
}

async fn current_issue(stack: &HermeticRealStack) -> Issue {
    stack
        .forge()
        .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
        .await
        .expect("issue lookup")
        .expect("source issue")
}

async fn assert_cancelled_trace(stack: &HermeticRealStack, cx: &skein::cx::Cx) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let runs = stack.trace_runs().expect("trace journal query");
        if runs.len() == 1 && runs[0].summary.status == AgentTraceRunStatus::Cancelled {
            let terminal = runs[0].events.last().expect("terminal trace event");
            assert!(matches!(
                &terminal.event,
                AgentActivityEventV1::RunFinished(RunFinishedV1 {
                    status: RunStatusV1::Cancelled,
                    ..
                })
            ));
            assert!(
                runs.iter()
                    .all(|run| run.summary.status != AgentTraceRunStatus::Active),
                "forwarding must not leave an active journal run"
            );
            let local = stack.local_trace_runs().expect("local trace spool");
            assert_eq!(local.len(), 1);
            assert!(local[0].acknowledged_seq >= terminal.seq);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for one terminal cancelled trace: {:?}",
            runs.iter()
                .map(|run| run.summary.status)
                .collect::<Vec<_>>()
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

fn assert_no_agent_effects(snapshot: HermeticActivitySnapshot) {
    assert_eq!(snapshot, HermeticActivitySnapshot::default());
}

async fn assert_no_product_effects(stack: &HermeticRealStack) {
    assert_eq!(
        stack.origin_branches(stack.primary_repo_path()).unwrap(),
        vec!["main".to_string()],
        "a fenced attempt cannot push a product branch"
    );
    assert!(
        stack
            .pull_requests()
            .await
            .expect("pull request inventory")
            .is_empty(),
        "a fenced attempt cannot submit an implementation PR"
    );
    let checkout = stack
        .workspace_checkout(stack.primary_repo_path())
        .expect("prepared checkout");
    let output = Command::new("git")
        .args(["-C", checkout.to_str().unwrap(), "status", "--porcelain"])
        .output()
        .expect("inspect workspace git status");
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "a fenced attempt cannot mutate its checkout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

async fn await_pause(cx: &skein::cx::Cx, pause: PausePermit, description: &str) -> ReachedPause {
    skein::time::timeout(
        temper_engine_io::runtime::timer_now(cx),
        Duration::from_secs(20),
        Box::pin(pause.arrived()),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
}

async fn wait_for_outbox_count(stack: &HermeticRealStack, cx: &skein::cx::Cx, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let count = stack.pending_result_count().expect("read result outbox");
        if count == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} outbox entries, saw {count}"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(1)).await;
    }
}
