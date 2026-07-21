use std::process::Command;
use std::time::{Duration, Instant};

use temper_engine::AgentTraceRunStatus;
use temper_forge_model::{Forge, Issue, IssueState, UpdateIssue};
use temper_protocol_activity::{AgentActivityEventV1, RunFinishedV1, RunStatusV1};
use temper_testing::real_stack::{
    FakeModelResponse, HermeticActivitySnapshot, HermeticIssueSpec, HermeticRealStack,
    HermeticRealStackBuilder, PausePermit, PausePoint, ReachedPause,
};
use temper_workflow::{parse_metadata_block, replace_metadata_block};

use super::{OwnershipLoss, RecoveredAttempt};

pub(super) async fn ownership_world(handle: &skein::runtime::RuntimeHandle) -> HermeticRealStack {
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

pub(super) async fn start_recovered_attempt(
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

pub(super) async fn mutate_durable_ownership(
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

pub(super) async fn current_issue(stack: &HermeticRealStack) -> Issue {
    stack
        .forge()
        .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
        .await
        .expect("issue lookup")
        .expect("source issue")
}

pub(super) fn assert_cancelled_journal(stack: &HermeticRealStack) -> u64 {
    let runs = stack.trace_runs().expect("trace journal query");
    assert_eq!(runs.len(), 1, "one cancellation run is journaled");
    assert_eq!(runs[0].summary.status, AgentTraceRunStatus::Cancelled);
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
    terminal.seq
}

pub(super) async fn assert_cancelled_trace(stack: &HermeticRealStack, cx: &skein::cx::Cx) {
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

pub(super) fn assert_no_agent_effects(snapshot: HermeticActivitySnapshot) {
    assert_eq!(snapshot, HermeticActivitySnapshot::default());
}

pub(super) async fn assert_no_product_effects(stack: &HermeticRealStack) {
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

pub(super) async fn await_pause(
    cx: &skein::cx::Cx,
    pause: PausePermit,
    description: &str,
) -> ReachedPause {
    skein::time::timeout(
        temper_engine_io::runtime::timer_now(cx),
        Duration::from_secs(20),
        Box::pin(pause.arrived()),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
}

pub(super) async fn wait_for_outbox_count(
    stack: &HermeticRealStack,
    cx: &skein::cx::Cx,
    expected: usize,
) {
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
