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

/// Drive a machine against an explicit clock instead of the ambient runtime
/// one. This is the deterministic test/simulation entry point: feed a recorded
/// completion sequence with synthetic [`EngineTime`] stamps and assert on the
/// emitted requests, with no runtime, no sleeps, and no races. The lab runtime
/// uses the same seam with a virtual clock.
///
/// `clock` is invoked once per delivery to stamp the transition, mirroring the
/// production drive loop's once-per-delivery snapshot.
pub fn drive_sync<M, X, C>(machine: &mut M, executor: &X, completions: Vec<M::Completion>, mut clock: C)
where
    M: Machine,
    X: Executor<M>,
    C: FnMut() -> EngineTime,
{
    for request in machine.on_start(clock()) {
        executor.execute(request);
    }
    if machine.is_stopped() {
        return;
    }
    for completion in completions {
        for request in machine.on_completion(clock(), completion) {
            executor.execute(request);
        }
        if machine.is_stopped() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A trivial ping/pong machine: each `Tick` completion emits one `Pong`
    /// request and stops after `limit` ticks. Exercises `drive_sync` with no
    /// runtime.
    struct PingPong {
        seen: u32,
        limit: u32,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Req {
        Pong(u32),
    }

    impl Machine for PingPong {
        type Completion = ();
        type Request = Req;

        fn on_start(&mut self, _now: EngineTime) -> Vec<Req> {
            vec![Req::Pong(0)]
        }

        fn on_completion(&mut self, _now: EngineTime, _completion: ()) -> Vec<Req> {
            self.seen += 1;
            vec![Req::Pong(self.seen)]
        }

        fn is_stopped(&self) -> bool {
            self.seen >= self.limit
        }
    }

    struct Recorder {
        requests: RefCell<Vec<Req>>,
    }

    impl Executor<PingPong> for Recorder {
        fn execute(&self, request: Req) {
            self.requests.borrow_mut().push(request);
        }
    }

    #[test]
    fn drive_sync_runs_start_then_completions_until_stopped() {
        let mut machine = PingPong { seen: 0, limit: 2 };
        let recorder = Recorder {
            requests: RefCell::new(Vec::new()),
        };
        // Three completions offered, but the machine stops after 2 ticks.
        drive_sync(
            &mut machine,
            &recorder,
            vec![(), (), ()],
            || EngineTime::ZERO,
        );
        assert_eq!(
            *recorder.requests.borrow(),
            vec![Req::Pong(0), Req::Pong(1), Req::Pong(2)]
        );
        assert_eq!(machine.seen, 2);
    }

    #[test]
    fn drive_sync_stops_immediately_when_started_stopped() {
        let mut machine = PingPong { seen: 5, limit: 2 };
        let recorder = Recorder {
            requests: RefCell::new(Vec::new()),
        };
        drive_sync(&mut machine, &recorder, vec![()], || EngineTime::ZERO);
        // on_start still ran once; the loop then exits before any completion.
        assert_eq!(*recorder.requests.borrow(), vec![Req::Pong(0)]);
    }
}
