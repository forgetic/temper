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
