use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Poll, Waker};

use temper_protocol_activity::CaptureModeV1;

use crate::config::WorkerAgentTraceConfig;

/// One atomically observed point in the clone-shared forwarding state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TraceCoordinationSnapshot {
    pub append_generation: u64,
    pub acknowledgement_generation: u64,
}

/// One coalesced run notification and the newest append it represents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyTraceRun {
    pub run_id: String,
    pub generation: u64,
}

/// An atomic drain of dirty run IDs at one append generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyTraceRuns {
    pub generation: u64,
    pub runs: Vec<DirtyTraceRun>,
}

#[derive(Default)]
struct TraceCoordinationState {
    append_generation: u64,
    acknowledgement_generation: u64,
    dirty_runs: BTreeMap<String, u64>,
    active_runs: BTreeSet<String>,
    next_waiter_id: u64,
    append_waiters: BTreeMap<u64, Waker>,
    acknowledgement_waiters: BTreeMap<u64, Waker>,
}

#[derive(Default)]
pub(super) struct TraceCoordination {
    state: Mutex<TraceCoordinationState>,
}

impl std::fmt::Debug for TraceCoordination {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.lock();
        formatter
            .debug_struct("TraceCoordination")
            .field("append_generation", &state.append_generation)
            .field(
                "acknowledgement_generation",
                &state.acknowledgement_generation,
            )
            .field("dirty_runs", &state.dirty_runs)
            .field("active_runs", &state.active_runs)
            .finish_non_exhaustive()
    }
}

impl TraceCoordination {
    pub(super) fn register_active(&self, run_id: &str) {
        let inserted = self.lock().active_runs.insert(run_id.to_string());
        debug_assert!(inserted, "trace run registered active more than once");
    }

    pub(super) fn unregister_active(&self, run_id: &str) {
        self.lock().active_runs.remove(run_id);
    }

    pub(super) fn is_active(&self, run_id: &str) -> bool {
        self.lock().active_runs.contains(run_id)
    }

    pub(super) fn snapshot(&self) -> TraceCoordinationSnapshot {
        let state = self.lock();
        TraceCoordinationSnapshot {
            append_generation: state.append_generation,
            acknowledgement_generation: state.acknowledgement_generation,
        }
    }

    pub(super) fn drain_dirty_runs(&self) -> DirtyTraceRuns {
        let mut state = self.lock();
        let generation = state.append_generation;
        let runs = std::mem::take(&mut state.dirty_runs)
            .into_iter()
            .map(|(run_id, generation)| DirtyTraceRun { run_id, generation })
            .collect();
        DirtyTraceRuns { generation, runs }
    }

    pub(super) fn publish_append(&self, run_id: &str) {
        let waiters = {
            let mut state = self.lock();
            state.append_generation = next_generation(state.append_generation);
            let generation = state.append_generation;
            state.dirty_runs.insert(run_id.to_string(), generation);
            std::mem::take(&mut state.append_waiters)
        };
        wake_all(waiters.into_values());
    }

    pub(super) fn publish_acknowledgement(&self) {
        let waiters = {
            let mut state = self.lock();
            state.acknowledgement_generation = next_generation(state.acknowledgement_generation);
            std::mem::take(&mut state.acknowledgement_waiters)
        };
        wake_all(waiters.into_values());
    }

    pub(super) async fn wait_for_append(&self, after: u64) -> u64 {
        let mut registration = TraceWaiterRegistration::new(self, TraceWaiterKind::Append);
        std::future::poll_fn(|cx| {
            let mut state = self.lock();
            if state.append_generation != after {
                registration.remove_locked(&mut state);
                return Poll::Ready(state.append_generation);
            }
            registration.register_locked(&mut state, cx.waker());
            Poll::Pending
        })
        .await
    }

    pub(super) async fn wait_for_acknowledgement(&self, after: u64) -> u64 {
        let mut registration = TraceWaiterRegistration::new(self, TraceWaiterKind::Acknowledgement);
        std::future::poll_fn(|cx| {
            let mut state = self.lock();
            if state.acknowledgement_generation != after {
                registration.remove_locked(&mut state);
                return Poll::Ready(state.acknowledgement_generation);
            }
            registration.register_locked(&mut state, cx.waker());
            Poll::Pending
        })
        .await
    }

    #[cfg(test)]
    pub(super) fn append_waiter_count(&self) -> usize {
        self.lock().append_waiters.len()
    }

    fn lock(&self) -> MutexGuard<'_, TraceCoordinationState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Clone, Copy)]
enum TraceWaiterKind {
    Append,
    Acknowledgement,
}

struct TraceWaiterRegistration<'a> {
    coordination: &'a TraceCoordination,
    kind: TraceWaiterKind,
    id: Option<u64>,
}

impl<'a> TraceWaiterRegistration<'a> {
    fn new(coordination: &'a TraceCoordination, kind: TraceWaiterKind) -> Self {
        Self {
            coordination,
            kind,
            id: None,
        }
    }

    fn register_locked(&mut self, state: &mut TraceCoordinationState, waker: &Waker) {
        let id = *self.id.get_or_insert_with(|| {
            state.next_waiter_id = next_generation(state.next_waiter_id);
            state.next_waiter_id
        });
        let waiters = match self.kind {
            TraceWaiterKind::Append => &mut state.append_waiters,
            TraceWaiterKind::Acknowledgement => &mut state.acknowledgement_waiters,
        };
        if waiters
            .get(&id)
            .is_none_or(|registered| !registered.will_wake(waker))
        {
            waiters.insert(id, waker.clone());
        }
    }

    fn remove_locked(&mut self, state: &mut TraceCoordinationState) {
        let Some(id) = self.id.take() else {
            return;
        };
        match self.kind {
            TraceWaiterKind::Append => state.append_waiters.remove(&id),
            TraceWaiterKind::Acknowledgement => state.acknowledgement_waiters.remove(&id),
        };
    }
}

impl Drop for TraceWaiterRegistration<'_> {
    fn drop(&mut self) {
        if self.id.is_some() {
            let mut state = self.coordination.lock();
            self.remove_locked(&mut state);
        }
    }
}

/// Factory for new worker-stamped runs and restart recovery.
///
/// Clones retain both the durable configuration and the in-memory forwarding
/// coordination. Producers and their forwarder must use clones of one
/// collector so a durable append cannot race past forwarder waiter setup.
#[derive(Clone, Debug)]
pub struct TraceCollector {
    pub(super) config: WorkerAgentTraceConfig,
    pub(super) coordination: Arc<TraceCoordination>,
}

impl Default for TraceCollector {
    fn default() -> Self {
        Self::new(WorkerAgentTraceConfig::default())
    }
}

impl TraceCollector {
    pub fn new(config: WorkerAgentTraceConfig) -> Self {
        Self {
            config,
            coordination: Arc::new(TraceCoordination::default()),
        }
    }

    /// Whether this collector is configured to require durable run traces.
    /// Capture `off` is the sole explicit no-trace compatibility case.
    pub fn tracing_enabled(&self) -> bool {
        self.config.policy.capture != CaptureModeV1::Off
    }

    pub(crate) fn forwarding_enabled(&self) -> bool {
        self.tracing_enabled() && self.config.spool_root.is_some()
    }

    /// Atomically snapshots append and acknowledgement generations.
    pub fn coordination_snapshot(&self) -> TraceCoordinationSnapshot {
        self.coordination.snapshot()
    }

    /// Drains coalesced dirty run IDs with each run's newest append generation.
    pub fn drain_dirty_runs(&self) -> DirtyTraceRuns {
        self.coordination.drain_dirty_runs()
    }

    /// Waits until the append generation differs from `after`.
    ///
    /// Generation comparison and waker registration share one lock, so an
    /// append between a scan and this call cannot be missed.
    pub async fn wait_for_append(&self, after: u64) -> u64 {
        self.coordination.wait_for_append(after).await
    }

    /// Waits until a durable acknowledgement changes after `after`.
    pub async fn wait_for_acknowledgement(&self, after: u64) -> u64 {
        self.coordination.wait_for_acknowledgement(after).await
    }

    #[cfg(test)]
    pub(super) fn append_waiter_count(&self) -> usize {
        self.coordination.append_waiter_count()
    }
}

fn next_generation(generation: u64) -> u64 {
    generation
        .checked_add(1)
        .expect("trace forwarding generation exhausted")
}

fn wake_all(waiters: impl IntoIterator<Item = Waker>) {
    for waiter in waiters {
        waiter.wake();
    }
}
