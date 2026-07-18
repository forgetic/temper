use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use temper_protocol_activity::AgentActivityCapturePolicyV1;

use super::tests::{context, usage_frame};
use super::*;
use crate::config::WorkerAgentTraceConfig;

fn collector(root: &Path) -> TraceCollector {
    TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(root.to_path_buf()),
    })
}

#[test]
fn cloned_collectors_coalesce_each_run_at_its_newest_generation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let observer = collector.clone();
    let run = collector
        .begin_run("coalesced-run", &context())
        .expect("begin")
        .expect("enabled");
    run.accept_frame(usage_frame(1)).expect("append usage");
    run.finish_success(None).expect("finish");

    let drained = observer.drain_dirty_runs();
    assert_eq!(drained.generation, 3);
    assert_eq!(drained.runs.len(), 1);
    assert_eq!(drained.runs[0].run_id, run.run_id());
    assert_eq!(drained.runs[0].generation, 3);
    assert_eq!(collector.coordination_snapshot().append_generation, 3);
    assert!(collector.drain_dirty_runs().runs.is_empty());
}

#[test]
fn append_between_snapshot_and_waiter_registration_is_observed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let waiter = collector.clone();
    let before = waiter.coordination_snapshot().append_generation;

    // Publish before the wait future is even created. Its first poll must see
    // the changed generation instead of sleeping forever.
    let run = collector
        .begin_run("append-before-wait", &context())
        .expect("begin")
        .expect("enabled");
    let observed = temper_worker_io::block_on(async move { waiter.wait_for_append(before).await });
    assert_eq!(observed, 1);
    assert_eq!(collector.drain_dirty_runs().runs[0].run_id, run.run_id());
}

#[test]
fn registered_append_waiters_wake_and_cancel_without_leaking() {
    struct CountingWake(AtomicUsize);

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let waiter = collector.clone();
    let mut waiting = Box::pin(waiter.wait_for_append(0));
    let wake_count = Arc::new(CountingWake(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&wake_count));
    let mut cx = Context::from_waker(&waker);
    assert!(waiting.as_mut().poll(&mut cx).is_pending());
    assert_eq!(collector.append_waiter_count(), 1);

    collector
        .begin_run("wake-registered-waiter", &context())
        .expect("begin")
        .expect("enabled");
    assert_eq!(wake_count.0.load(Ordering::SeqCst), 1);
    assert!(matches!(waiting.as_mut().poll(&mut cx), Poll::Ready(1)));
    drop(waiting);

    let mut cancelled = Box::pin(waiter.wait_for_append(1));
    assert!(cancelled.as_mut().poll(&mut cx).is_pending());
    assert_eq!(collector.append_waiter_count(), 1);
    drop(cancelled);
    assert_eq!(collector.append_waiter_count(), 0);
}

#[test]
fn failed_append_does_not_advance_or_wake_forwarding() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            max_inline_bytes: 1,
            max_blob_bytes: 1,
            max_run_bytes: 5_000,
            ..Default::default()
        },
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("quota-failure", &context())
        .expect("begin")
        .expect("enabled");
    let mut token = 1;
    loop {
        let before = collector.coordination_snapshot().append_generation;
        match run.accept_frame(usage_frame(token)) {
            Ok(_) => token += 1,
            Err(TraceError::QuotaExceeded) => {
                assert_eq!(collector.coordination_snapshot().append_generation, before);
                break;
            }
            Err(error) => panic!("unexpected append failure: {error}"),
        }
    }
}

#[test]
fn durable_acknowledgements_publish_one_shared_generation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let observer = collector.clone();
    let run = collector
        .begin_run("acknowledged-run", &context())
        .expect("begin")
        .expect("enabled");
    let sequence = run.finish_success(None).expect("finish");
    let run_id = run.run_id().to_string();
    drop(run);
    let before = observer.coordination_snapshot().acknowledgement_generation;

    collector
        .acknowledge(&run_id, sequence)
        .expect("durable acknowledgement");
    let acknowledgement_waiter = observer.clone();
    assert_eq!(
        temper_worker_io::block_on(async move {
            acknowledgement_waiter
                .wait_for_acknowledgement(before)
                .await
        }),
        before + 1
    );
    let acknowledgement_observer = observer.clone();
    let awaited_run_id = run_id.clone();
    assert!(temper_worker_io::block_on(async move {
        acknowledgement_observer
            .await_acknowledged(&awaited_run_id, sequence, Duration::from_millis(50))
            .await
    }));

    collector
        .acknowledge(&run_id, sequence)
        .expect("idempotent acknowledgement");
    assert_eq!(
        observer.coordination_snapshot().acknowledgement_generation,
        before + 1
    );
}
