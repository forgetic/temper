// SPDX-License-Identifier: MPL-2.0

//! Production lifecycle for periodic engine-owned agent-trace retention.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::Duration;

use temper_engine::{AgentTraceJournal, Daemon};
use temper_engine_io::{OneshotReceiver, Spawner, oneshot};

/// Production delay between retention passes. Startup performs its own pass,
/// so the periodic task waits one interval before running again.
pub const AGENT_TRACE_RETENTION_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Default)]
struct TaskCancellation {
    cancelled: Arc<AtomicBool>,
    waiters: Arc<Mutex<Vec<Waker>>>,
}

impl TaskCancellation {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        for waiter in std::mem::take(&mut *self.waiters.lock().expect("retention cancel lock")) {
            waiter.wake();
        }
    }

    async fn run<F: Future>(&self, future: F) -> Option<F::Output> {
        let mut future = std::pin::pin!(future);
        std::future::poll_fn(|cx| {
            if self.cancelled.load(Ordering::SeqCst) {
                return Poll::Ready(None);
            }
            if let Poll::Ready(output) = Pin::new(&mut future).poll(cx) {
                return Poll::Ready(Some(output));
            }
            let mut waiters = self.waiters.lock().expect("retention cancel lock");
            if self.cancelled.load(Ordering::SeqCst) {
                return Poll::Ready(None);
            }
            waiters.push(cx.waker().clone());
            Poll::Pending
        })
        .await
    }
}

/// Joinable control for the periodic retention component.
///
/// Dropping the handle requests cancellation. Production shutdown calls
/// [`stop`](Self::stop) so any in-progress filesystem pass also finishes before
/// the runtime is drained.
pub struct TraceRetentionTask {
    cancellation: TaskCancellation,
    joined: Option<OneshotReceiver<()>>,
}

impl TraceRetentionTask {
    pub async fn stop(mut self) {
        self.cancellation.cancel();
        if let Some(joined) = self.joined.take() {
            let _ = joined.recv().await;
        }
    }
}

impl Drop for TraceRetentionTask {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

/// Starts the periodic journal-retention component used by split and standalone
/// wiring. `cadence` is injectable so runtime tests can advance the lifecycle
/// without sleeping for the production hour.
pub fn spawn_trace_retention_task(
    spawner: &Arc<dyn Spawner>,
    daemon: Daemon,
    journal: AgentTraceJournal,
    cadence: Duration,
) -> TraceRetentionTask {
    let cadence = cadence.max(Duration::from_millis(1));
    let cancellation = TaskCancellation::default();
    let task_cancellation = cancellation.clone();
    let (joined_tx, joined) = oneshot();
    spawner.spawn_with_cx(move |cx| async move {
        loop {
            if task_cancellation
                .run(temper_engine_io::runtime::sleep_for(&cx, cadence))
                .await
                .is_none()
            {
                break;
            }
            let Some(protection) = task_cancellation
                .run(daemon.trace_retention_protection())
                .await
                .flatten()
            else {
                if task_cancellation.cancelled.load(Ordering::SeqCst) {
                    break;
                }
                tracing::warn!(
                    target: "temper::engine",
                    service = "engine",
                    event = "agent.activity.retention_skipped",
                    "agent trace retention could not snapshot in-flight assignments; preserving all runs for this pass"
                );
                continue;
            };
            let cleanup_journal = journal.clone();
            // Once a filesystem pass starts, shutdown joins it instead of
            // dropping the blocking join future and leaving cleanup detached.
            let cleanup = skein::runtime::spawn_blocking(move || {
                cleanup_journal.cleanup_retention(&protection)
            })
            .await;
            match cleanup {
                Ok(report) => {
                    for failure in report.failures {
                        tracing::warn!(
                            target: "temper::engine",
                            service = "engine",
                            event = "agent.activity.retention_run_failed",
                            run_directory = %failure.run_directory,
                            error = %failure.error,
                            "agent trace retention skipped one run and will continue"
                        );
                    }
                }
                Err(error) => tracing::warn!(
                    target: "temper::engine",
                    service = "engine",
                    event = "agent.activity.retention_failed",
                    %error,
                    "agent trace retention pass failed; the periodic task will continue"
                ),
            }
        }
        joined_tx.send(());
    });
    TraceRetentionTask {
        cancellation,
        joined: Some(joined),
    }
}
