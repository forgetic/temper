// SPDX-License-Identifier: MPL-2.0

//! Runtime bootstrap helpers for engine binaries and tests.

use std::future::Future;

use skein::cx::Cx;
use skein::runtime::reactor::create_reactor;
use skein::runtime::{Runtime, RuntimeBuilder, RuntimeHandle};

/// An skein runtime configured for temper services: I/O reactor attached
/// and a small blocking pool for filesystem/git helpers.
pub struct EngineRuntime {
    runtime: Runtime,
}

impl EngineRuntime {
    /// Handle for spawning tasks onto this runtime.
    pub fn handle(&self) -> RuntimeHandle {
        self.runtime.handle()
    }

    /// Run a future to completion on the current thread.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }
}

/// Build the production engine runtime.
///
/// Deliberately single-threaded (libuv-shaped): one loop thread runs every
/// task, so while a machine transition executes, nothing else in the engine
/// progresses — concurrency without parallelism. Requests are still handled
/// concurrently (cooperatively interleaved at await points); the heavy work
/// lives in child processes and remote services, so the loop thread is far
/// from saturation in practice. Blocking work must go through
/// `spawn_blocking` (its small pool is separate, like libuv's). If a shard
/// ever saturates, prefer partitioning into more machines over re-enabling
/// worker parallelism — the serialized core wouldn't benefit from threads.
pub fn build_runtime() -> Result<EngineRuntime, String> {
    let reactor =
        create_reactor().map_err(|error| format!("creating skein reactor failed: {error}"))?;
    let runtime = RuntimeBuilder::current_thread()
        .blocking_threads(1, 4)
        .with_reactor(reactor)
        .build()
        .map_err(|error| format!("building skein runtime failed: {error}"))?;
    Ok(EngineRuntime { runtime })
}

/// Build a runtime and run one future to completion **as a task**. The body
/// receives no capabilities — code that needs the task's [`Cx`] (timers,
/// process deadlines) or a spawner must use [`block_on_with`]. This is the
/// standard entry for bodies that only call APIs taking their capabilities
/// explicitly:
///
/// ```text
/// #[test]
/// fn my_async_test() {
///     temper_io_engine::block_on(async { ... });
/// }
/// ```
///
/// Panics from the future are propagated to the caller.
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    block_on_with(move |_cx, _handle| future)
}

/// [`block_on`] on an already-built runtime.
pub fn block_on_runtime<F>(runtime: &EngineRuntime, future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    block_on_runtime_with(runtime, move |_cx, _handle| future)
}

/// Build a runtime and run one future to completion as a task, handing the
/// body its capabilities **explicitly**: the root task's [`Cx`] (the clock
/// capability — pass it to anything computing deadlines) and the runtime's
/// [`RuntimeHandle`] (the spawn capability; it implements
/// [`crate::spawn::Spawner`], so `Arc::new(handle)` coerces to
/// `Arc<dyn Spawner>` wherever one is required). There is no ambient way to
/// recover either — this signature is the only source.
///
/// ```text
/// temper_io_engine::block_on_with(|cx, handle| async move { ... });
/// ```
pub fn block_on_with<F, Fut>(f: F) -> Fut::Output
where
    F: FnOnce(Cx, RuntimeHandle) -> Fut + Send + 'static,
    Fut: Future + Send + 'static,
    Fut::Output: Send + 'static,
{
    let runtime = build_runtime().expect("build skein runtime");
    block_on_runtime_with(&runtime, f)
}

/// [`block_on_with`] on an already-built runtime.
pub fn block_on_runtime_with<F, Fut>(runtime: &EngineRuntime, f: F) -> Fut::Output
where
    F: FnOnce(Cx, RuntimeHandle) -> Fut + Send + 'static,
    Fut: Future + Send + 'static,
    Fut::Output: Send + 'static,
{
    let (result_tx, result_rx) = crate::queue::oneshot();
    let handle = runtime.handle();
    runtime.handle().spawn_with_cx(move |cx| async move {
        let mut future = Box::pin(f(cx, handle));
        let outcome = std::future::poll_fn(move |task_cx| {
            let poll = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                future.as_mut().poll(task_cx)
            }));
            match poll {
                Ok(std::task::Poll::Ready(value)) => std::task::Poll::Ready(Ok(value)),
                Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
                Err(payload) => std::task::Poll::Ready(Err(payload)),
            }
        })
        .await;
        result_tx.send(outcome);
    });
    match runtime.block_on(result_rx.recv()) {
        Some(Ok(value)) => value,
        Some(Err(payload)) => std::panic::resume_unwind(payload),
        None => panic!("engine task vanished without a result"),
    }
}

/// The current time on the clock that actually fires timers.
///
/// Deadlines must be computed against the runtime's timer-driver clock;
/// `Cx::now()` is the logical clock, whose epoch can drift from the wall
/// timer wheel and skew every sleep/timeout computed from it.
pub fn timer_now(cx: &Cx) -> skein::types::Time {
    // Prefer the timer-driver clock (the one that actually fires timers). The
    // driverless fallback uses the process wall clock — the same base
    // skein's fallback timing thread checks driverless sleeps against —
    // rather than the logical Cx clock, whose epoch can skew from the wheel.
    cx.timer_driver()
        .map_or_else(skein::time::wall_now, |driver| driver.now())
}

/// Sleep helper for shell and test code running inside an engine task.
/// (Machines never sleep — they request timers.) The caller's task `Cx`
/// supplies the clock that deadlines are computed against.
pub async fn sleep_for(cx: &Cx, duration: std::time::Duration) {
    skein::time::sleep(timer_now(cx), duration).await;
}
