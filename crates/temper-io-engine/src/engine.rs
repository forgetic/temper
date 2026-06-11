// SPDX-License-Identifier: MPL-2.0

//! The engine loop: completions in, pure transition, requests out.

use crate::machine::{EngineTime, Machine};
use crate::queue::CqReceiver;

/// The imperative shell: executes one `<io-event-request>` produced by the
/// machine. Implementations hold the runtime handle, the completion-queue
/// sender, and any I/O clients they need; `execute` must not block — anything
/// asynchronous is spawned, and its result is submitted to the completion
/// queue as a new `<io-event-completion>`.
pub trait Executor<M: Machine + ?Sized> {
    fn execute(&self, request: M::Request);
}

/// Drive a machine: deliver completions one at a time and execute the
/// requests each transition produces. Exits when the machine reports itself
/// stopped (after a shutdown completion) or when every completion sender is
/// gone, i.e. no I/O can ever complete again.
///
/// This loop is the only place where the functional core and the imperative
/// shell meet, and the only place that reads a clock for the core: the
/// runtime's monotonic clock is snapshotted exactly once per delivery and
/// handed to the transition as data. The machine it returns can be inspected
/// by the shell for teardown decisions.
///
/// Must run inside an engine task (it reads the task's capability context for
/// the runtime clock) — spawn it, or run it under
/// [`crate::runtime::block_on`].
pub async fn drive<M, X>(
    mut machine: M,
    executor: &X,
    mut completions: CqReceiver<M::Completion>,
) -> M
where
    M: Machine,
    X: Executor<M>,
{
    let cx = crate::runtime::current_cx();
    let now = || EngineTime::from(crate::runtime::timer_now(&cx));

    for request in machine.on_start(now()) {
        executor.execute(request);
    }
    if machine.is_stopped() {
        return machine;
    }
    while let Some(completion) = completions.recv().await {
        for request in machine.on_completion(now(), completion) {
            executor.execute(request);
        }
        if machine.is_stopped() {
            break;
        }
    }
    machine
}
