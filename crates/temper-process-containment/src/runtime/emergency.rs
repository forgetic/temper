use std::collections::BTreeMap;
use std::io;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{
    CleanupReport, ContainmentBackendKind, ContainmentIdentity, ContainmentRootIdentity,
    ContainmentScope,
};

/// Maximum number of boundary dispatch results retained in one emergency receipt
/// or registry snapshot. Every registered boundary is still dispatched to.
pub const MAX_EMERGENCY_TERMINATION_EVIDENCE: usize = 256;

/// Monotonic out-of-band escalation requested for a live containment boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmergencyEscalation {
    ForcedTermination,
    HardKill,
}

/// Whether an emergency command was accepted by its independent backend owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmergencyDispatchOutcome {
    Dispatched,
    OwnerUnavailable,
}

/// Identifies one live process-containment boundary without inspecting members.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmergencyTerminationBoundary {
    identity: ContainmentIdentity,
    scope: ContainmentScope,
    backend: ContainmentBackendKind,
    root: ContainmentRootIdentity,
    root_pid: u32,
}

impl EmergencyTerminationBoundary {
    pub fn identity(&self) -> &ContainmentIdentity {
        &self.identity
    }

    pub fn scope(&self) -> &ContainmentScope {
        &self.scope
    }

    pub fn backend(&self) -> ContainmentBackendKind {
        self.backend
    }

    pub fn root(&self) -> &ContainmentRootIdentity {
        &self.root
    }

    pub fn root_pid(&self) -> u32 {
        self.root_pid
    }
}

/// Bounded evidence that one emergency command was queued. This is dispatch
/// evidence only; it is deliberately not recursive-empty or reap proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmergencyBoundaryDispatch {
    boundary: EmergencyTerminationBoundary,
    outcome: EmergencyDispatchOutcome,
}

impl EmergencyBoundaryDispatch {
    pub fn boundary(&self) -> &EmergencyTerminationBoundary {
        &self.boundary
    }

    pub fn outcome(&self) -> EmergencyDispatchOutcome {
        self.outcome
    }
}

/// Bounded result of dispatching one escalation to every registered boundary.
///
/// A receipt never implies process exit and cannot be converted into a
/// [`CleanupReport`]. Ordinary cleanup remains responsible for direct-child
/// reap and recursive-empty proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmergencyEscalationReceipt {
    escalation: EmergencyEscalation,
    dispatched: Vec<EmergencyBoundaryDispatch>,
    omitted: usize,
}

impl EmergencyEscalationReceipt {
    pub fn escalation(&self) -> EmergencyEscalation {
        self.escalation
    }

    pub fn dispatched(&self) -> &[EmergencyBoundaryDispatch] {
        &self.dispatched
    }

    pub fn omitted(&self) -> usize {
        self.omitted
    }

    pub fn requested_count(&self) -> usize {
        self.dispatched.len().saturating_add(self.omitted)
    }
}

/// Bounded point-in-time view of process boundaries that have not yet produced
/// ordinary proof-based cleanup completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmergencyTerminationSnapshot {
    boundaries: Vec<EmergencyTerminationBoundary>,
    omitted: usize,
}

impl EmergencyTerminationSnapshot {
    pub fn boundaries(&self) -> &[EmergencyTerminationBoundary] {
        &self.boundaries
    }

    pub fn omitted(&self) -> usize {
        self.omitted
    }

    pub fn registered_count(&self) -> usize {
        self.boundaries.len().saturating_add(self.omitted)
    }

    pub fn is_empty(&self) -> bool {
        self.registered_count() == 0
    }
}

pub(crate) trait EmergencyDispatcher: Send + Sync {
    /// Implementations must only enqueue/write a command and return promptly.
    fn dispatch(&self, escalation: EmergencyEscalation) -> io::Result<()>;
}

/// Cloneable out-of-band command path owned independently of ordinary cleanup.
#[derive(Clone)]
pub struct EmergencyTerminationHandle {
    dispatcher: Arc<dyn EmergencyDispatcher>,
}

impl EmergencyTerminationHandle {
    pub(crate) fn new(dispatcher: Arc<dyn EmergencyDispatcher>) -> Self {
        Self { dispatcher }
    }

    /// Creates a handle for an externally owned backend command loop. Sending
    /// on this unbounded channel is a non-blocking dispatch; the receiver must
    /// remain independent of ordinary cleanup.
    pub fn from_sender(sender: Sender<EmergencyEscalation>) -> Self {
        Self::new(Arc::new(SenderDispatcher(sender)))
    }

    /// Builds two independent backend-owner threads. A blocked forced
    /// termination operation therefore cannot prevent a later hard-kill
    /// command from being consumed.
    pub(crate) fn from_owners<F, H>(name: &str, forced: F, hard_kill: H) -> io::Result<Self>
    where
        F: FnMut() -> io::Result<()> + Send + 'static,
        H: FnMut() -> io::Result<()> + Send + 'static,
    {
        let (forced_tx, forced_rx) = mpsc::channel();
        let (hard_kill_tx, hard_kill_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name(format!("temper-{name}-forced-termination"))
            .spawn(move || run_owner(forced_rx, forced))?;
        std::thread::Builder::new()
            .name(format!("temper-{name}-hard-kill"))
            .spawn(move || run_owner(hard_kill_rx, hard_kill))?;
        Ok(Self::new(Arc::new(ChannelDispatcher {
            forced: forced_tx,
            hard_kill: hard_kill_tx,
        })))
    }

    fn dispatch(&self, escalation: EmergencyEscalation) -> EmergencyDispatchOutcome {
        if self.dispatcher.dispatch(escalation).is_ok() {
            EmergencyDispatchOutcome::Dispatched
        } else {
            EmergencyDispatchOutcome::OwnerUnavailable
        }
    }
}

fn run_owner(receiver: mpsc::Receiver<()>, mut operation: impl FnMut() -> io::Result<()>) {
    while receiver.recv().is_ok() {
        let _ = operation();
    }
}

struct SenderDispatcher(Sender<EmergencyEscalation>);

impl EmergencyDispatcher for SenderDispatcher {
    fn dispatch(&self, escalation: EmergencyEscalation) -> io::Result<()> {
        self.0
            .send(escalation)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "emergency owner closed"))
    }
}

struct ChannelDispatcher {
    forced: Sender<()>,
    hard_kill: Sender<()>,
}

impl EmergencyDispatcher for ChannelDispatcher {
    fn dispatch(&self, escalation: EmergencyEscalation) -> io::Result<()> {
        let result = match escalation {
            EmergencyEscalation::ForcedTermination => self.forced.send(()),
            EmergencyEscalation::HardKill => self.hard_kill.send(()),
        };
        result.map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "emergency owner closed"))
    }
}

#[derive(Clone, Default)]
pub struct EmergencyTerminationRegistry {
    inner: Arc<RegistryInner>,
}

#[derive(Default)]
struct RegistryInner {
    state: Mutex<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    next_id: u64,
    boundaries: BTreeMap<u64, RegisteredBoundary>,
}

#[derive(Clone)]
struct RegisteredBoundary {
    boundary: EmergencyTerminationBoundary,
    handle: EmergencyTerminationHandle,
}

impl EmergencyTerminationRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue forced termination on every live backend owner without acquiring a
    /// process cleanup coordinator or performing member discovery.
    pub fn request_forced_termination(&self) -> EmergencyEscalationReceipt {
        self.request(EmergencyEscalation::ForcedTermination)
    }

    /// Queue the strongest available kill on every live backend owner without
    /// acquiring a process cleanup coordinator or performing member discovery.
    pub fn request_hard_kill(&self) -> EmergencyEscalationReceipt {
        self.request(EmergencyEscalation::HardKill)
    }

    pub fn snapshot(&self) -> EmergencyTerminationSnapshot {
        let state = lock_unpoisoned(&self.inner.state);
        let total = state.boundaries.len();
        let boundaries = state
            .boundaries
            .values()
            .take(MAX_EMERGENCY_TERMINATION_EVIDENCE)
            .map(|registered| registered.boundary.clone())
            .collect::<Vec<_>>();
        EmergencyTerminationSnapshot {
            omitted: total.saturating_sub(boundaries.len()),
            boundaries,
        }
    }

    fn request(&self, escalation: EmergencyEscalation) -> EmergencyEscalationReceipt {
        // Clone handles under the registry mutex and release it before backend
        // dispatch. Dispatch can never contend with registration or ordinary
        // proof-based deregistration.
        let registered = {
            let state = lock_unpoisoned(&self.inner.state);
            state.boundaries.values().cloned().collect::<Vec<_>>()
        };
        let total = registered.len();
        let mut dispatched = Vec::with_capacity(total.min(MAX_EMERGENCY_TERMINATION_EVIDENCE));
        for registered in registered {
            let outcome = registered.handle.dispatch(escalation);
            if dispatched.len() < MAX_EMERGENCY_TERMINATION_EVIDENCE {
                dispatched.push(EmergencyBoundaryDispatch {
                    boundary: registered.boundary,
                    outcome,
                });
            }
        }
        EmergencyEscalationReceipt {
            escalation,
            omitted: total.saturating_sub(dispatched.len()),
            dispatched,
        }
    }

    pub(super) fn register(
        &self,
        identity: ContainmentIdentity,
        scope: ContainmentScope,
        backend: ContainmentBackendKind,
        root: ContainmentRootIdentity,
        root_pid: u32,
        handle: EmergencyTerminationHandle,
    ) -> EmergencyRegistration {
        let mut state = lock_unpoisoned(&self.inner.state);
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        state.boundaries.insert(
            id,
            RegisteredBoundary {
                boundary: EmergencyTerminationBoundary {
                    identity,
                    scope,
                    backend,
                    root,
                    root_pid,
                },
                handle,
            },
        );
        EmergencyRegistration {
            registry: self.clone(),
            id,
        }
    }
}

pub(super) struct EmergencyRegistration {
    registry: EmergencyTerminationRegistry,
    id: u64,
}

impl EmergencyRegistration {
    /// Remove only after the caller has constructed a valid ordinary cleanup
    /// report. Keeping this check here prevents future call sites from turning
    /// emergency dispatch into synthetic quiescence.
    pub(super) fn complete(&self, report: &CleanupReport) {
        assert!(report.proves_quiescence());
        lock_unpoisoned(&self.registry.inner.state)
            .boundaries
            .remove(&self.id);
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
