//! Runtime bootstrap helpers for engine binaries and tests.

use std::cell::RefCell;
use std::future::Future;

use skein::cx::Cx;
use skein::runtime::reactor::create_reactor;
use skein::runtime::{Runtime, RuntimeBuilder, RuntimeHandle};

/// Shared, clearable slot holding the runtime handle for [`current_handle`].
///
/// `Mutex<Option<…>>` rather than `OnceLock` so [`EngineRuntime`]'s `Drop` can
/// clear it: the slot is captured by the runtime's `on_thread_start` callback
/// (stored in the runtime config), so a handle left inside would form an
/// `Arc` cycle that keeps the runtime alive forever.
type HandleSlot = std::sync::Arc<std::sync::Mutex<Option<RuntimeHandle>>>;

thread_local! {
    /// Ambient engine-runtime handle slot for the current thread.
    ///
    /// skein deliberately dropped `Runtime::current_handle()` from its public
    /// API (consumers must thread handles explicitly); anvil's engine layer
    /// reinstates the ambient handle *for itself only*: [`build_runtime`]
    /// registers an `on_thread_start` callback that installs this slot on
    /// every thread the runtime drives (workers and `block_on` callers), and
    /// fills the slot with the handle once the runtime exists. Any code polled
    /// on the engine (shells spawning sub-agent tasks) can then recover the
    /// handle via [`current_handle`]. Lab-runtime tasks never pass through
    /// these entry points, so the handle stays absent there — the lab harness
    /// keeps its explicit Spawner seam.
    static CURRENT_HANDLE_SLOT: RefCell<Option<HandleSlot>> = const { RefCell::new(None) };
}

/// The engine-runtime handle ambient to the current thread, if any.
///
/// Resolves on threads driven by a [`build_runtime`] runtime — worker threads
/// and `block_on` callers — while that runtime is alive. Returns `None` on
/// unrelated threads, inside lab-runtime tasks, and after the
/// [`EngineRuntime`] has been dropped.
#[must_use]
pub fn current_handle() -> Option<RuntimeHandle> {
    CURRENT_HANDLE_SLOT.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|slot| slot.lock().unwrap_or_else(|e| e.into_inner()).clone())
    })
}

/// An skein runtime configured for anvil services: I/O reactor attached
/// and a small blocking pool for filesystem/git helpers.
pub struct EngineRuntime {
    runtime: Runtime,
    /// The ambient-handle slot shared with this runtime's threads (see
    /// [`current_handle`]); cleared on drop to break the `Arc` cycle through
    /// the runtime config.
    handle_slot: HandleSlot,
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

impl Drop for EngineRuntime {
    fn drop(&mut self) {
        self.handle_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
}

/// Build the production engine runtime.
///
/// Deliberately single-threaded (libuv-shaped): one loop thread runs every
/// task, so while a machine transition executes, nothing else in the engine
/// progresses — concurrency without parallelism. Requests are still handled
/// concurrently (cooperatively interleaved at await points); the heavy work
/// lives in child LLM calls and remote services, so the loop thread is far
/// from saturation in practice. Blocking work must go through `spawn_blocking`
/// (its small pool is separate, like libuv's). If a worker ever saturates,
/// prefer partitioning into more machines over re-enabling worker parallelism
/// — the serialized core wouldn't benefit from threads.
pub fn build_runtime() -> Result<EngineRuntime, String> {
    let reactor =
        create_reactor().map_err(|error| format!("creating skein reactor failed: {error}"))?;
    let handle_slot: HandleSlot = std::sync::Arc::default();
    let thread_slot = std::sync::Arc::clone(&handle_slot);
    let runtime = RuntimeBuilder::current_thread()
        .blocking_threads(1, 4)
        .with_reactor(reactor)
        .on_thread_start(move || {
            // Install the ambient-handle slot on every thread this runtime
            // drives, so `current_handle()` resolves inside engine tasks.
            let slot = std::sync::Arc::clone(&thread_slot);
            CURRENT_HANDLE_SLOT.with(|cell| *cell.borrow_mut() = Some(slot));
        })
        .build()
        .map_err(|error| format!("building skein runtime failed: {error}"))?;
    handle_slot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(runtime.handle());
    Ok(EngineRuntime {
        runtime,
        handle_slot,
    })
}

/// Build a runtime and run one future to completion **as a task**, so the
/// body has an ambient [`Cx`] (timers, HTTP calls, and process deadlines all
/// need one). This is the standard `main()` entry for anvil engine binaries
/// and the test-harness replacement for `#[tokio::test]` bodies:
///
/// ```text
/// #[test]
/// fn my_async_test() {
///     anvil_io_engine::block_on(async { ... });
/// }
/// ```
///
/// Panics from the future are propagated to the caller.
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let runtime = build_runtime().expect("build skein runtime");
    block_on_runtime(&runtime, future)
}

/// [`block_on`] on an already-built runtime.
pub fn block_on_runtime<F>(runtime: &EngineRuntime, future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (result_tx, result_rx) = crate::queue::oneshot();
    runtime.handle().spawn_with_cx(move |_cx| async move {
        let mut future = Box::pin(future);
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
/// Deadlines must be computed against the runtime's timer-driver clock; with
/// no ambient driver the process wall clock is the base driverless sleeps are
/// checked against, so deadlines stay on the clock that fires them.
pub fn timer_now(cx: &Cx) -> skein::types::Time {
    cx.timer_driver()
        .map_or_else(skein::time::wall_now, |driver| driver.now())
}

/// Engine-clock "now" usable from any thread.
///
/// Inside an engine task this is the ambient timer-driver clock (the one that
/// fires timers). Outside one — e.g. a raw `EngineRuntime::block_on` future —
/// it is the process wall clock, which is the same base driverless sleeps are
/// checked against (they fire via skein's fallback timing thread), so
/// deadlines stay consistent in both contexts.
pub fn engine_now() -> skein::types::Time {
    Cx::current().map_or_else(skein::time::wall_now, |cx| timer_now(&cx))
}

/// Sleep helper for shell and test code running inside an engine task.
/// (Machines never sleep — they request timers.)
pub async fn sleep_for(duration: std::time::Duration) {
    skein::time::sleep(engine_now(), duration).await;
}

/// The ambient capability context of the current skein task.
///
/// The scheduler installs each task's `Cx` while polling it, so this is
/// always available inside spawned tasks. It panics outside of one — engine
/// executors only call it from task context.
pub fn current_cx() -> Cx {
    Cx::current().expect("called outside an skein task")
}

/// The ambient task context if present, or a detached root context.
///
/// The detached context is never cancelled and carries an infinite budget —
/// exactly the semantics non-cancellable shell I/O had under the previous
/// runtime. It carries no clock: never use it for timers; use a spawned
/// task's `Cx` for time.
pub fn ambient_cx() -> Cx {
    Cx::current().unwrap_or_else(Cx::for_testing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_runs_future_and_returns_value() {
        let value = block_on(async { 1 + 2 });
        assert_eq!(value, 3);
    }

    #[test]
    fn block_on_has_ambient_cx_for_timers() {
        // sleep_for needs an ambient clock; if block_on didn't run the body as
        // a task with a Cx, this would panic.
        let woke = block_on(async {
            sleep_for(std::time::Duration::from_millis(1)).await;
            true
        });
        assert!(woke);
    }

    #[test]
    #[should_panic(expected = "engine task panicked: boom")]
    fn block_on_propagates_panics() {
        block_on(async { panic!("engine task panicked: boom") });
    }
}
