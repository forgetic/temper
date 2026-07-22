//! Dedicated OS watchdog for the absolute standalone shutdown deadline.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use temper_worker::WorkerEmergencyShutdownHandle;

use super::StandaloneShutdownDeadline;

trait WatchdogClock: Send + Sync + 'static {
    fn now(&self) -> Instant;
    /// Returns true when the deadline was reached, false when disarmed.
    fn wait_until(&self, deadline: Instant, disarmed: &AtomicBool) -> bool;
    fn wake(&self);
}

#[derive(Default)]
struct SystemWatchdogClock {
    gate: Mutex<()>,
    wake: Condvar,
}

impl WatchdogClock for SystemWatchdogClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn wait_until(&self, deadline: Instant, disarmed: &AtomicBool) -> bool {
        let mut gate = self
            .gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if disarmed.load(Ordering::Acquire) {
                return false;
            }
            let remaining = deadline.saturating_duration_since(self.now());
            if remaining.is_zero() {
                return true;
            }
            let waited = self
                .wake
                .wait_timeout(gate, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            gate = waited.0;
        }
    }

    fn wake(&self) {
        self.wake.notify_all();
    }
}

trait ProcessTerminator: Send + Sync + 'static {
    fn terminate(&self);
}

struct AbortProcess;

impl ProcessTerminator for AbortProcess {
    fn terminate(&self) {
        // `abort` neither unwinds nor runs process-owner drops. That is required
        // on bounded crash handoff because an owner drop may itself be the
        // operation that blocked the single-threaded runtime.
        std::process::abort();
    }
}

/// Two pre-armed OS threads keep emergency descendant KILL independent from
/// final process termination. In particular, a blocked registry/backend KILL
/// dispatch can never prevent the absolute-deadline thread from calling the
/// no-unwind terminator.
pub(super) struct StandaloneShutdownCoordinator {
    disarmed: Arc<AtomicBool>,
    clock: Arc<dyn WatchdogClock>,
    watchdogs: Vec<JoinHandle<()>>,
}

impl StandaloneShutdownCoordinator {
    pub(super) fn arm(
        deadline: StandaloneShutdownDeadline,
        emergency: WorkerEmergencyShutdownHandle,
    ) -> Result<Self, String> {
        Self::arm_with(
            deadline,
            Arc::new(SystemWatchdogClock::default()),
            Arc::new(AbortProcess),
            Arc::new(move || emergency.request_emergency_kill()),
        )
    }

    fn arm_with(
        deadline: StandaloneShutdownDeadline,
        clock: Arc<dyn WatchdogClock>,
        terminator: Arc<dyn ProcessTerminator>,
        emergency_kill: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let disarmed = Arc::new(AtomicBool::new(false));

        let emergency_disarmed = Arc::clone(&disarmed);
        let emergency_clock = Arc::clone(&clock);
        let emergency_watchdog = std::thread::Builder::new()
            .name("temper-standalone-emergency-kill".to_string())
            .spawn(move || {
                if emergency_clock.wait_until(deadline.emergency_kill_at(), &emergency_disarmed) {
                    emergency_kill();
                }
            })
            .map_err(|error| {
                format!("failed to arm standalone emergency KILL watchdog: {error}")
            })?;

        let termination_disarmed = Arc::clone(&disarmed);
        let termination_clock = Arc::clone(&clock);
        let termination_watchdog = match std::thread::Builder::new()
            .name("temper-standalone-absolute-deadline".to_string())
            .spawn(move || {
                if termination_clock.wait_until(deadline.absolute_deadline(), &termination_disarmed)
                {
                    terminator.terminate();
                }
            }) {
            Ok(watchdog) => watchdog,
            Err(error) => {
                disarmed.store(true, Ordering::Release);
                clock.wake();
                let _ = emergency_watchdog.join();
                return Err(format!(
                    "failed to arm standalone absolute-deadline watchdog: {error}"
                ));
            }
        };

        Ok(Self {
            disarmed,
            clock,
            watchdogs: vec![emergency_watchdog, termination_watchdog],
        })
    }

    /// This is intentionally the only disarm API. The caller consumes the
    /// coordinator only after worker, daemon admission, trace retention,
    /// assignment release, and HTTP drain have all supplied join proof.
    pub(super) fn disarm(mut self) -> Result<(), String> {
        self.disarmed.store(true, Ordering::Release);
        self.clock.wake();
        for watchdog in self.watchdogs.drain(..) {
            watchdog
                .join()
                .map_err(|_| "standalone shutdown watchdog panicked".to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    struct FakeClock {
        now: Mutex<Instant>,
        wake: Condvar,
    }

    impl FakeClock {
        fn new(now: Instant) -> Self {
            Self {
                now: Mutex::new(now),
                wake: Condvar::new(),
            }
        }

        fn advance_to(&self, now: Instant) {
            *self.now.lock().unwrap() = now;
            self.wake.notify_all();
        }
    }

    impl WatchdogClock for FakeClock {
        fn now(&self) -> Instant {
            *self.now.lock().unwrap()
        }

        fn wait_until(&self, deadline: Instant, disarmed: &AtomicBool) -> bool {
            let mut now = self.now.lock().unwrap();
            loop {
                if disarmed.load(Ordering::Acquire) {
                    return false;
                }
                if *now >= deadline {
                    return true;
                }
                now = self
                    .wake
                    .wait(now)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        }

        fn wake(&self) {
            self.wake.notify_all();
        }
    }

    struct RecordingTerminator(Arc<AtomicUsize>);

    impl ProcessTerminator for RecordingTerminator {
        fn terminate(&self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn wait_for(counter: &AtomicUsize, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while counter.load(Ordering::Acquire) != expected {
            assert!(Instant::now() < deadline, "watchdog action did not run");
            std::thread::yield_now();
        }
    }

    #[test]
    fn absolute_deadline_terminates_even_when_emergency_dispatch_blocks() {
        let signal = Instant::now();
        let deadline = StandaloneShutdownDeadline::from_signal(signal, Duration::from_secs(30))
            .expect("deadline");
        assert_eq!(deadline.signal_received_at(), signal);
        assert_eq!(
            deadline.absolute_deadline(),
            signal + Duration::from_secs(30)
        );
        assert_eq!(
            deadline.emergency_kill_at(),
            signal + Duration::from_secs(25)
        );

        let clock = Arc::new(FakeClock::new(signal));
        let terminated = Arc::new(AtomicUsize::new(0));
        let emergency_started = Arc::new(AtomicUsize::new(0));
        let release_emergency = Arc::new((Mutex::new(false), Condvar::new()));
        let coordinator = StandaloneShutdownCoordinator::arm_with(
            deadline,
            clock.clone(),
            Arc::new(RecordingTerminator(Arc::clone(&terminated))),
            {
                let emergency_started = Arc::clone(&emergency_started);
                let release_emergency = Arc::clone(&release_emergency);
                Arc::new(move || {
                    emergency_started.fetch_add(1, Ordering::AcqRel);
                    let (lock, wake) = &*release_emergency;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = wake.wait(released).unwrap();
                    }
                })
            },
        )
        .expect("arm watchdogs");

        clock.advance_to(deadline.emergency_kill_at());
        wait_for(&emergency_started, 1);
        clock.advance_to(deadline.absolute_deadline());
        wait_for(&terminated, 1);

        let (lock, wake) = &*release_emergency;
        *lock.lock().unwrap() = true;
        wake.notify_all();
        coordinator.disarm().expect("join watchdogs");
    }

    #[test]
    fn proven_graceful_shutdown_disarms_both_watchdogs() {
        let signal = Instant::now();
        let deadline = StandaloneShutdownDeadline::from_signal(signal, Duration::from_secs(30))
            .expect("deadline");
        let clock = Arc::new(FakeClock::new(signal));
        let terminated = Arc::new(AtomicUsize::new(0));
        let emergency = Arc::new(AtomicUsize::new(0));
        let coordinator = StandaloneShutdownCoordinator::arm_with(
            deadline,
            clock.clone(),
            Arc::new(RecordingTerminator(Arc::clone(&terminated))),
            {
                let emergency = Arc::clone(&emergency);
                Arc::new(move || {
                    emergency.fetch_add(1, Ordering::AcqRel);
                })
            },
        )
        .expect("arm watchdogs");

        coordinator.disarm().expect("disarm");
        clock.advance_to(deadline.absolute_deadline());
        assert_eq!(emergency.load(Ordering::Acquire), 0);
        assert_eq!(terminated.load(Ordering::Acquire), 0);
    }
}
