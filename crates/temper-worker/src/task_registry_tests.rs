use std::future::Future;
use std::task::{Context, Poll, Waker};

use super::*;

fn task(job_id: &str, generation: u64) -> ActiveJobTask {
    ActiveJobTask::new(
        job_id,
        format!("attempt-{generation}"),
        generation,
        AttemptFence::open(),
        JobCancellation::default(),
    )
}

fn poll_once(future: &mut std::pin::Pin<Box<dyn Future<Output = ()>>>) -> Poll<()> {
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    future.as_mut().poll(&mut cx)
}

#[test]
fn concurrent_jobs_join_only_after_the_last_task_leaves() {
    let registry = WorkerTaskRegistry::new();
    let first = task("job-a", 1);
    let second = task("job-b", 2);
    assert!(registry.register(first.clone()));
    assert!(registry.register(second.clone()));
    registry.mark_running(&first);
    registry.mark_running(&second);

    let notification = registry.begin_shutdown(WorkerShutdown::Graceful);
    let mut joined: std::pin::Pin<Box<dyn Future<Output = ()>>> = Box::pin(notification.wait());
    assert!(poll_once(&mut joined).is_pending());

    registry.mark_joined(&second);
    assert!(poll_once(&mut joined).is_pending());
    assert_eq!(registry.active_jobs()[0].job_id(), "job-a");

    registry.mark_joined(&first);
    assert!(poll_once(&mut joined).is_ready());
    assert!(registry.is_empty());
}

#[test]
fn graceful_shutdown_fences_and_cancels_every_active_attempt() {
    let registry = WorkerTaskRegistry::new();
    let first = task("job-a", 1);
    let second = task("job-b", 2);
    assert!(registry.register(first.clone()));
    assert!(registry.register(second.clone()));

    let notification = registry.begin_shutdown(WorkerShutdown::Graceful);

    assert!(!notification.is_ready());
    for active in registry.active_jobs() {
        assert!(!active.fence().is_open());
        assert_eq!(
            active.cancellation().requested(),
            Some(JobCancellationRequest::Graceful)
        );
        assert_eq!(
            active.join_state(),
            ActiveJobJoinState::CancellationRequested
        );
    }
    assert!(!registry.register(task("job-c", 3)));
}

#[test]
fn cleanup_pending_keeps_the_registry_join_blocked() {
    let registry = WorkerTaskRegistry::new();
    let active = task("blocked", 7);
    assert!(registry.register(active.clone()));
    registry.mark_cleanup_pending("blocked", "attempt-7", 7);
    assert_eq!(
        registry.active_jobs()[0].join_state(),
        ActiveJobJoinState::CleanupPending
    );

    let notification = registry.begin_shutdown(WorkerShutdown::Graceful);
    let mut joined: std::pin::Pin<Box<dyn Future<Output = ()>>> = Box::pin(notification.wait());
    assert!(poll_once(&mut joined).is_pending());

    registry.request_all(JobCancellationRequest::ForcedTermination);
    registry.request_all(JobCancellationRequest::HardKill);
    assert!(poll_once(&mut joined).is_pending());

    registry.mark_joined(&active);
    assert!(poll_once(&mut joined).is_ready());
}

#[test]
fn crash_immediately_hard_escalates_and_preserves_publication_fence() {
    let registry = WorkerTaskRegistry::new();
    let active = task("restart", 11);
    assert!(registry.register(active.clone()));

    registry.begin_shutdown(WorkerShutdown::Crash);

    let snapshot = registry.active_jobs().pop().expect("active task");
    assert!(!snapshot.fence().is_open());
    assert_eq!(
        snapshot.cancellation().requested(),
        Some(JobCancellationRequest::HardKill)
    );
    assert_eq!(snapshot.join_state(), ActiveJobJoinState::HardKillRequested);
}

#[test]
fn exact_cancellation_closes_fence_and_updates_join_state_monotonically() {
    let registry = WorkerTaskRegistry::new();
    let active = task("owned", 9);
    let fence = active.fence().clone();
    let cancellation = active.cancellation().clone();
    assert!(registry.register(active.clone()));
    registry.mark_running(&active);

    assert!(!registry.cancel_attempt("owned", "stale-attempt", 9));
    assert!(fence.is_open());
    assert_eq!(cancellation.requested(), None);

    assert!(registry.cancel_attempt("owned", "attempt-9", 9));
    assert!(!fence.is_open(), "the exact fence closes synchronously");
    assert_eq!(
        cancellation.requested(),
        Some(JobCancellationRequest::Graceful)
    );
    assert_eq!(
        registry.active_jobs()[0].join_state(),
        ActiveJobJoinState::CancellationRequested
    );

    assert!(registry.request_attempt("owned", "attempt-9", 9, JobCancellationRequest::HardKill,));
    assert!(registry.cancel_attempt("owned", "attempt-9", 9));
    assert_eq!(
        registry.active_jobs()[0].join_state(),
        ActiveJobJoinState::HardKillRequested,
        "duplicate cooperative cancellation must not regress escalation"
    );
    assert_eq!(
        cancellation.requested(),
        Some(JobCancellationRequest::HardKill)
    );
}

#[test]
fn typed_trace_blocker_retains_attempt_and_trace_identity() {
    let registry = WorkerTaskRegistry::new();
    let active = task("trace", 13);
    assert!(registry.register(active));
    registry.mark_shutdown_blocker(
        "trace",
        "attempt-13",
        13,
        ShutdownBlocker::new(
            ShutdownBlockerKind::TerminalTraceAck,
            ShutdownEscalationStage::Graceful,
            "agent_trace",
            "awaiting_acknowledgement",
        )
        .with_trace(Some("run-13"), Some(27)),
    );
    registry.begin_shutdown(WorkerShutdown::Graceful);
    registry.request_all(JobCancellationRequest::HardKill);

    let blockers = registry.active_jobs()[0].shutdown_blockers(
        "worker-13",
        ShutdownEscalationStage::HardKill,
        Instant::now() + Duration::from_secs(1),
    );
    let trace = blockers
        .iter()
        .find(|blocker| blocker.kind == ShutdownBlockerKind::TerminalTraceAck)
        .expect("terminal trace blocker");
    assert_eq!(trace.worker_id.as_deref(), Some("worker-13"));
    assert_eq!(trace.job_id.as_deref(), Some("trace"));
    assert_eq!(trace.attempt_id.as_deref(), Some("attempt-13"));
    assert_eq!(trace.trace_run_id.as_deref(), Some("run-13"));
    assert_eq!(trace.trace_sequence, Some(27));
}

#[test]
fn component_tasks_are_counted_by_shutdown_kind() {
    let tasks = WorkerComponentTasks::default();
    let _delivery = tasks
        .register(WorkerComponentTaskKind::ResultDelivery)
        .expect("delivery guard");
    let _recording = tasks
        .register(WorkerComponentTaskKind::ResultRecordingAcknowledgement)
        .expect("recording guard");
    let _transport = tasks
        .register(WorkerComponentTaskKind::Transport)
        .expect("transport guard");
    let _background = tasks
        .register(WorkerComponentTaskKind::BackgroundComponent)
        .expect("background guard");

    let blockers = tasks.shutdown_blockers(
        "worker-components",
        ShutdownEscalationStage::HardKill,
        Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(blockers.len(), 4);
    assert!(blockers.iter().any(|blocker| {
        blocker.kind == ShutdownBlockerKind::ResultDelivery
            && blocker.owner_name == "result_delivery"
    }));
    for expected in [
        "result_recording_acknowledgement",
        "transport",
        "background_component",
    ] {
        assert!(blockers.iter().any(|blocker| {
            blocker.kind == ShutdownBlockerKind::ComponentTask && blocker.owner_name == expected
        }));
    }
}

#[test]
fn shutdown_suppresses_active_attempt_terminal_publication() {
    let registry = WorkerTaskRegistry::new();
    let active = task("restart", 12);
    assert!(registry.register(active.clone()));
    registry.begin_shutdown(WorkerShutdown::Crash);

    let mut published = true;
    registry.finish_with(&active, |allowed| published = allowed);

    assert!(!published);
    assert!(registry.is_empty());
}
