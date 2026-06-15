//! Per-process cadence drivers: poll loop and wakeable poll loop.

use super::clock::ManualClock;
use super::report::{DriveError, RunReport, WorkerRunReport};
use crate::Worker;
use crate::trigger::{TriggerScheduler, WakeTarget};
use chrono::{DateTime, Duration, Utc};
use std::time::Instant;
use temper_forge_model::{ChangeHint, ChangeSource, ChangeSourceEvent};

#[derive(Clone, Debug)]
enum PollClock {
    System,
    Manual(ManualClock),
}

impl PollClock {
    fn now(&self) -> DateTime<Utc> {
        match self {
            Self::System => Utc::now(),
            Self::Manual(clock) => clock.now(),
        }
    }

    fn after_interval(&self, interval: Duration) {
        if let Self::Manual(clock) = self {
            clock.advance(interval);
        }
    }
}

/// Per-process cadence driver for one worker.
///
/// The loop is deliberately trigger-source agnostic: the only trigger today is
/// the poll interval, and a future `ChangeSource` can replace or shorten the
/// wait without changing worker logic. The default constructor uses wall-clock
/// time for `Worker::tick`; tests can inject a [`ManualClock`] with
/// [`with_clock`](Self::with_clock).
pub struct PollLoop<'a> {
    worker: &'a dyn Worker,
    poll_interval: Duration,
    clock: PollClock,
}

impl<'a> PollLoop<'a> {
    /// Creates a poll loop using wall-clock timestamps.
    pub fn new(worker: &'a dyn Worker, poll_interval: Duration) -> Self {
        Self {
            worker,
            poll_interval,
            clock: PollClock::System,
        }
    }

    /// Creates a poll loop using a deterministic manual clock.
    pub fn with_clock(worker: &'a dyn Worker, poll_interval: Duration, clock: ManualClock) -> Self {
        Self {
            worker,
            poll_interval,
            clock: PollClock::Manual(clock),
        }
    }

    /// Runs until `stop_signal` returns `true`.
    ///
    /// The stop signal is checked before each tick and again before sleeping,
    /// so shutdown after a successful tick does not wait for another interval.
    pub async fn run_until<S>(&self, mut stop_signal: S) -> Result<RunReport, DriveError>
    where
        S: FnMut() -> bool,
    {
        let mut report = self.empty_report();
        while !stop_signal() {
            self.tick_once(&mut report).await?;
            if stop_signal() {
                break;
            }
            std::thread::sleep(poll_sleep_duration(self.poll_interval));
            self.clock.after_interval(self.poll_interval);
        }
        Ok(report)
    }

    /// Ticks the worker `count` times without sleeping.
    ///
    /// This deterministic helper is intended for tests and stage sketches. With
    /// a manual clock it still advances the clock by the poll interval between
    /// ticks, preserving cadence semantics without wall-clock delay.
    pub async fn run_bounded(&self, count: u64) -> Result<RunReport, DriveError> {
        let mut report = self.empty_report();
        for index in 0..count {
            self.tick_once(&mut report).await?;
            if index + 1 < count {
                self.clock.after_interval(self.poll_interval);
            }
        }
        Ok(report)
    }

    /// Returns the poll interval configured for this loop.
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    fn empty_report(&self) -> RunReport {
        RunReport {
            ticks: 0,
            workers: vec![WorkerRunReport {
                name: self.worker.name().to_string(),
                ticks: 0,
                actions: 0,
            }],
        }
    }

    async fn tick_once(&self, report: &mut RunReport) -> Result<(), DriveError> {
        let progress =
            self.worker
                .tick(self.clock.now())
                .await
                .map_err(|source| DriveError::Worker {
                    worker: self.worker.name().to_string(),
                    source,
                })?;
        report.ticks = report.ticks.saturating_add(1);
        let worker = &mut report.workers[0];
        worker.ticks = worker.ticks.saturating_add(1);
        worker.actions = worker.actions.saturating_add(u64::from(progress.actions));
        Ok(())
    }
}

/// Per-worker cadence driver with an optional hint accelerator.
///
/// The worker still ticks immediately once, then after each poll interval as a
/// liveness backstop. Hints from a companion [`ChangeSource`] are routed through
/// [`TriggerScheduler`] and can wake the same tick path before the poll deadline.
pub struct WakeablePollLoop<'a> {
    worker: &'a dyn Worker,
    target: WakeTarget,
    poll_interval: Duration,
    debounce: Duration,
    clock: PollClock,
}

impl<'a> WakeablePollLoop<'a> {
    /// Creates a wakeable loop with no debounce, using wall-clock timestamps.
    pub fn new(worker: &'a dyn Worker, target: WakeTarget, poll_interval: Duration) -> Self {
        Self {
            worker,
            target,
            poll_interval,
            debounce: Duration::zero(),
            clock: PollClock::System,
        }
    }

    /// Creates a wakeable loop with a deterministic manual clock.
    pub fn with_clock(
        worker: &'a dyn Worker,
        target: WakeTarget,
        poll_interval: Duration,
        clock: ManualClock,
    ) -> Self {
        Self {
            worker,
            target,
            poll_interval,
            debounce: Duration::zero(),
            clock: PollClock::Manual(clock),
        }
    }

    /// Sets the scheduler debounce window used for incoming hints.
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    /// Runs until `stop_signal` returns `true`.
    ///
    /// `route` maps each hint to the workers it should wake. Hints that do not
    /// include this loop's target are ignored by this loop; broad routing is
    /// safe because every tick re-reads Forge state.
    pub async fn run_until<S, Stop, Route, Targets>(
        &self,
        source: &mut S,
        route: Route,
        mut stop_signal: Stop,
    ) -> Result<RunReport, DriveError>
    where
        S: ChangeSource,
        Stop: FnMut() -> bool,
        Route: Fn(&ChangeHint) -> Targets,
        Targets: IntoIterator<Item = WakeTarget>,
    {
        let mut report = self.empty_report();
        let mut scheduler = TriggerScheduler::new(self.debounce);
        let mut scheduled_tick = false;

        while !stop_signal() {
            self.tick_once(&mut report).await?;
            if scheduled_tick {
                scheduler.finish(&self.target, self.clock.now());
            }
            if stop_signal() {
                break;
            }
            scheduled_tick = self.wait_for_wake(source, &route, &mut scheduler, &mut stop_signal);
        }
        Ok(report)
    }

    fn wait_for_wake<S, Stop, Route, Targets>(
        &self,
        source: &mut S,
        route: &Route,
        scheduler: &mut TriggerScheduler,
        stop_signal: &mut Stop,
    ) -> bool
    where
        S: ChangeSource,
        Stop: FnMut() -> bool,
        Route: Fn(&ChangeHint) -> Targets,
        Targets: IntoIterator<Item = WakeTarget>,
    {
        let poll_due = Instant::now() + poll_sleep_duration(self.poll_interval);
        loop {
            if stop_signal() {
                return false;
            }
            let now = self.clock.now();
            if scheduler
                .due_targets(now)
                .into_iter()
                .any(|target| target == self.target)
            {
                return true;
            }
            let until_poll = poll_due.saturating_duration_since(Instant::now());
            if until_poll.is_zero() {
                self.clock.after_interval(self.poll_interval);
                return false;
            }
            let timeout = scheduler
                .next_due_at(&self.target)
                .map(|due_at| chrono_wait_duration(due_at, now).min(until_poll))
                .unwrap_or(until_poll);

            match source.recv_timeout(timeout) {
                ChangeSourceEvent::Hint(hint) => {
                    self.submit_hint(scheduler, route, hint);
                    self.drain_ready_hints(source, route, scheduler);
                }
                ChangeSourceEvent::Timeout => {}
                ChangeSourceEvent::Closed => std::thread::sleep(timeout),
            }
        }
    }

    fn drain_ready_hints<S, Route, Targets>(
        &self,
        source: &mut S,
        route: &Route,
        scheduler: &mut TriggerScheduler,
    ) where
        S: ChangeSource,
        Route: Fn(&ChangeHint) -> Targets,
        Targets: IntoIterator<Item = WakeTarget>,
    {
        while let ChangeSourceEvent::Hint(hint) = source.try_recv() {
            self.submit_hint(scheduler, route, hint);
        }
    }

    fn submit_hint<Route, Targets>(
        &self,
        scheduler: &mut TriggerScheduler,
        route: &Route,
        hint: ChangeHint,
    ) where
        Route: Fn(&ChangeHint) -> Targets,
        Targets: IntoIterator<Item = WakeTarget>,
    {
        scheduler.submit(&hint, route(&hint), self.clock.now());
    }

    fn empty_report(&self) -> RunReport {
        RunReport {
            ticks: 0,
            workers: vec![WorkerRunReport {
                name: self.worker.name().to_string(),
                ticks: 0,
                actions: 0,
            }],
        }
    }

    async fn tick_once(&self, report: &mut RunReport) -> Result<(), DriveError> {
        let progress =
            self.worker
                .tick(self.clock.now())
                .await
                .map_err(|source| DriveError::Worker {
                    worker: self.worker.name().to_string(),
                    source,
                })?;
        report.ticks = report.ticks.saturating_add(1);
        let worker = &mut report.workers[0];
        worker.ticks = worker.ticks.saturating_add(1);
        worker.actions = worker.actions.saturating_add(u64::from(progress.actions));
        Ok(())
    }
}

fn poll_sleep_duration(interval: Duration) -> std::time::Duration {
    interval.to_std().unwrap_or_default()
}

fn chrono_wait_duration(due_at: DateTime<Utc>, now: DateTime<Utc>) -> std::time::Duration {
    (due_at - now).to_std().unwrap_or_default()
}
