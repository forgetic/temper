//! Explicit, monotonic per-attempt cancellation handshake.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use temper_process_containment::{
    CleanupObservation, CleanupReport, CleanupSnapshot, CleanupTrigger,
    ContainmentCapabilityDiagnostic, ContainmentFallbackObservation,
};

/// Escalation requested by the worker-owned watchdog.
///
/// Values are ordered so a late or duplicate lower-severity request can never
/// undo an escalation that already reached an attempt owner.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum JobCancellationRequest {
    Graceful,
    ForcedTermination,
    HardKill,
}

impl JobCancellationRequest {
    const fn encoded(self) -> u8 {
        match self {
            Self::Graceful => 1,
            Self::ForcedTermination => 2,
            Self::HardKill => 3,
        }
    }

    fn decode(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Graceful),
            2 => Some(Self::ForcedTermination),
            3 => Some(Self::HardKill),
            _ => None,
        }
    }
}

/// How an attempt actually reached process quiescence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellationOutcome {
    Graceful,
    ForcedTermination,
    HardKill,
}

impl CancellationOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Graceful => "graceful",
            Self::ForcedTermination => "forced_termination",
            Self::HardKill => "hard_kill",
        }
    }

    pub const fn forced(self) -> bool {
        !matches!(self, Self::Graceful)
    }
}

/// Join proof for one attempt-owned thread or endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceJoinStatus {
    NotApplicable,
    Pending,
    Joined,
    Failed(String),
}

impl ResourceJoinStatus {
    pub fn is_proven(&self) -> bool {
        matches!(self, Self::NotApplicable | Self::Joined)
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Pending => "pending",
            Self::Joined => "joined",
            Self::Failed(_) => "failed",
        }
    }

    pub fn failure(&self) -> Option<&str> {
        match self {
            Self::Failed(message) => Some(message),
            _ => None,
        }
    }
}

/// Structured proof that every non-process resource at the agent boundary was
/// stopped and joined before the attempt became terminal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceJoinReport {
    pub process_supervisor: ResourceJoinStatus,
    pub stderr_reader: ResourceJoinStatus,
    pub lifecycle_endpoint: ResourceJoinStatus,
    pub activity_endpoint: ResourceJoinStatus,
    pub submit_endpoint: ResourceJoinStatus,
    pub forge_endpoint: ResourceJoinStatus,
    pub lifecycle_cancellation: ResourceJoinStatus,
}

impl ResourceJoinReport {
    pub fn no_process() -> Self {
        Self {
            process_supervisor: ResourceJoinStatus::NotApplicable,
            stderr_reader: ResourceJoinStatus::NotApplicable,
            lifecycle_endpoint: ResourceJoinStatus::NotApplicable,
            activity_endpoint: ResourceJoinStatus::NotApplicable,
            submit_endpoint: ResourceJoinStatus::NotApplicable,
            forge_endpoint: ResourceJoinStatus::NotApplicable,
            lifecycle_cancellation: ResourceJoinStatus::NotApplicable,
        }
    }

    pub fn all_joined(&self) -> bool {
        self.process_supervisor.is_proven()
            && self.stderr_reader.is_proven()
            && self.lifecycle_endpoint.is_proven()
            && self.activity_endpoint.is_proven()
            && self.submit_endpoint.is_proven()
            && self.forge_endpoint.is_proven()
            && self.lifecycle_cancellation.is_proven()
    }
}

/// The attempt owner's single terminal cleanup proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobCleanup {
    pub cancellation: Option<CancellationOutcome>,
    pub containment: CleanupReport,
    pub resources: ResourceJoinReport,
}

impl JobCleanup {
    pub fn no_process(cancellation: Option<CancellationOutcome>) -> Self {
        Self {
            cancellation,
            containment: CleanupReport::no_process(if cancellation.is_some() {
                CleanupTrigger::Cancellation
            } else {
                CleanupTrigger::NormalRootExit
            }),
            resources: ResourceJoinReport::no_process(),
        }
    }

    pub fn proves_quiescence(&self) -> bool {
        self.containment.proves_quiescence() && self.resources.all_joined()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum JobContainmentObservation {
    Cleanup(CleanupObservation),
    Snapshot(CleanupSnapshot),
    Capability(ContainmentCapabilityDiagnostic),
    Fallback(ContainmentFallbackObservation),
}

type CleanupSnapshotObserver = Arc<dyn Fn(JobContainmentObservation) + Send + Sync>;

#[derive(Default)]
struct JobCancellationState {
    request_waiters: Vec<Waker>,
    async_owners: usize,
    cleanup: Option<JobCleanup>,
    cleanup_observer: Option<CleanupSnapshotObserver>,
    containment_factory: Option<temper_process_containment::ContainmentFactory>,
}

/// Attempt-local cancellation handshake shared by the worker shell and every
/// owned effect.
///
/// `CancelJob` and both escalation stages monotonically update `request` and
/// wake the async process owner. The process owner records the real joined
/// cleanup report before returning. Drop remains only an abrupt-owner safety
/// net; the watchdog path does not rely on it.
#[derive(Clone, Default)]
pub struct JobCancellation {
    request: Arc<AtomicU8>,
    state: Arc<Mutex<JobCancellationState>>,
}

impl std::fmt::Debug for JobCancellation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JobCancellation")
            .field("requested", &self.requested())
            .field("cleanup", &self.cleanup())
            .finish_non_exhaustive()
    }
}

impl JobCancellation {
    /// Installs an instance-scoped containment factory for worker-owned child
    /// commands. Production attempts use automatic backend selection; compiled
    /// acceptance drivers use this seam to exercise the exact same owners with
    /// a forced supervisor or required delegated cgroup without global state.
    #[doc(hidden)]
    pub fn with_containment_factory(
        factory: temper_process_containment::ContainmentFactory,
    ) -> Self {
        let cancellation = Self::default();
        cancellation
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .containment_factory = Some(factory);
        cancellation
    }

    pub(crate) fn containment_factory(
        &self,
    ) -> Option<temper_process_containment::ContainmentFactory> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .containment_factory
            .clone()
    }

    /// Requests cooperative cancellation. Compatibility callers use this
    /// method; the worker shell maps `CancelJob` to the same request.
    pub fn cancel(&self) {
        self.request(JobCancellationRequest::Graceful);
    }

    pub fn force_terminate(&self) {
        self.request(JobCancellationRequest::ForcedTermination);
    }

    pub fn hard_kill(&self) {
        self.request(JobCancellationRequest::HardKill);
    }

    pub fn request(&self, requested: JobCancellationRequest) {
        let requested = requested.encoded();
        let mut current = self.request.load(Ordering::Acquire);
        loop {
            if current >= requested {
                return;
            }
            match self.request.compare_exchange_weak(
                current,
                requested,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
        let waiters = std::mem::take(
            &mut self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .request_waiters,
        );
        for waiter in waiters {
            waiter.wake();
        }
    }

    pub fn requested(&self) -> Option<JobCancellationRequest> {
        JobCancellationRequest::decode(self.request.load(Ordering::Acquire))
    }

    pub fn is_cancelled(&self) -> bool {
        self.requested().is_some()
    }

    pub async fn cancelled(&self) {
        std::future::poll_fn(|cx| {
            if self.is_cancelled() {
                return Poll::Ready(());
            }
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.is_cancelled() {
                return Poll::Ready(());
            }
            if !state
                .request_waiters
                .iter()
                .any(|waker| waker.will_wake(cx.waker()))
            {
                state.request_waiters.push(cx.waker().clone());
            }
            Poll::Pending
        })
        .await
    }

    /// Polls for the next request after `observed`. Intermediate escalation
    /// stages are never coalesced: if the shell publishes soft and hard
    /// escalation before the owner is polled again, the owner still observes
    /// Graceful, ForcedTermination, then HardKill in order.
    pub(crate) fn poll_request(
        &self,
        observed: Option<JobCancellationRequest>,
        cx: &mut Context<'_>,
    ) -> Poll<JobCancellationRequest> {
        if let Some(next) = self.next_request(observed) {
            return Poll::Ready(next);
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(next) = self.next_request(observed) {
            return Poll::Ready(next);
        }
        if !state
            .request_waiters
            .iter()
            .any(|waker| waker.will_wake(cx.waker()))
        {
            state.request_waiters.push(cx.waker().clone());
        }
        Poll::Pending
    }

    fn next_request(
        &self,
        observed: Option<JobCancellationRequest>,
    ) -> Option<JobCancellationRequest> {
        let published = self.requested()?;
        match observed {
            None => Some(JobCancellationRequest::Graceful),
            Some(JobCancellationRequest::Graceful)
                if published >= JobCancellationRequest::ForcedTermination =>
            {
                Some(JobCancellationRequest::ForcedTermination)
            }
            Some(JobCancellationRequest::ForcedTermination)
                if published >= JobCancellationRequest::HardKill =>
            {
                Some(JobCancellationRequest::HardKill)
            }
            _ => None,
        }
    }

    /// Installs the attempt-bound delivery used by containment observers. The
    /// callback is invoked outside the state lock so a completion queue may
    /// synchronously wake the worker machine.
    pub(crate) fn set_cleanup_observer(
        &self,
        observer: impl Fn(JobContainmentObservation) + Send + Sync + 'static,
    ) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cleanup_observer = Some(Arc::new(observer));
    }

    pub(crate) fn observe_cleanup(&self, snapshot: CleanupSnapshot) {
        self.observe_containment(JobContainmentObservation::Snapshot(snapshot));
    }

    pub(crate) fn observe_cleanup_observation(&self, observation: CleanupObservation) {
        self.observe_containment(JobContainmentObservation::Cleanup(observation));
    }

    pub(crate) fn observe_capability(&self, diagnostic: ContainmentCapabilityDiagnostic) {
        self.observe_containment(JobContainmentObservation::Capability(diagnostic));
    }

    pub(crate) fn observe_fallback(&self, fallback: ContainmentFallbackObservation) {
        self.observe_containment(JobContainmentObservation::Fallback(fallback));
    }

    fn observe_containment(&self, observation: JobContainmentObservation) {
        let observer = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cleanup_observer
            .clone();
        if let Some(observer) = observer {
            observer(observation);
        }
    }

    /// Records the joined supervisor report exactly once.
    pub(crate) fn record_cleanup(&self, cleanup: JobCleanup) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.cleanup.is_some() {
            return false;
        }
        state.cleanup = Some(cleanup);
        true
    }

    pub fn cleanup(&self) -> Option<JobCleanup> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cleanup
            .clone()
    }

    /// Registers an owner that must receive escalation requests and report
    /// quiescence asynchronously instead of being destroyed by the shell's
    /// compatibility cancellation race.
    pub fn register_async_owner(&self) -> JobCancellationOwner {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .async_owners += 1;
        JobCancellationOwner {
            cancellation: self.clone(),
        }
    }

    fn has_async_owner(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .async_owners
            > 0
    }

    /// Races cancellation for compatibility executors, but keeps an installed
    /// async owner alive so it can consume every escalation and report the real
    /// joined outcome.
    pub(crate) async fn run_to_quiescence<F: Future>(&self, future: F) -> Option<F::Output> {
        let mut future = std::pin::pin!(future);
        std::future::poll_fn(|cx| {
            if self.is_cancelled() && !self.has_async_owner() {
                return Poll::Ready(None);
            }
            if let Poll::Ready(output) = Pin::new(&mut future).poll(cx) {
                return Poll::Ready(Some(output));
            }
            if self.is_cancelled() && !self.has_async_owner() {
                return Poll::Ready(None);
            }
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.is_cancelled() && state.async_owners == 0 {
                return Poll::Ready(None);
            }
            if !state
                .request_waiters
                .iter()
                .any(|waker| waker.will_wake(cx.waker()))
            {
                state.request_waiters.push(cx.waker().clone());
            }
            Poll::Pending
        })
        .await
    }

    pub async fn run<F: Future>(&self, future: F) -> Option<F::Output> {
        let mut future = std::pin::pin!(future);
        std::future::poll_fn(|cx| {
            if self.is_cancelled() {
                return Poll::Ready(None);
            }
            if let Poll::Ready(output) = Pin::new(&mut future).poll(cx) {
                return Poll::Ready(Some(output));
            }
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.is_cancelled() {
                return Poll::Ready(None);
            }
            if !state
                .request_waiters
                .iter()
                .any(|waker| waker.will_wake(cx.waker()))
            {
                state.request_waiters.push(cx.waker().clone());
            }
            Poll::Pending
        })
        .await
    }
}

/// Shared bridge from process-containment observations into the attempt-bound
/// worker delivery. Every worker process owner uses this bridge so completed,
/// blocked, capability, and fallback evidence follow the same identity path.
pub(crate) struct JobCleanupObserver(pub(crate) JobCancellation);

impl temper_process_containment::CleanupObserver for JobCleanupObserver {
    fn observe(&self, snapshot: &CleanupSnapshot) {
        self.0.observe_cleanup(snapshot.clone());
    }

    fn observe_cleanup(&self, observation: &CleanupObservation) {
        self.0.observe_cleanup_observation(observation.clone());
    }

    fn observe_capability(&self, diagnostic: &ContainmentCapabilityDiagnostic) {
        self.0.observe_capability(diagnostic.clone());
    }

    fn observe_fallback(&self, fallback: &ContainmentFallbackObservation) {
        self.0.observe_fallback(fallback.clone());
    }
}

pub struct JobCancellationOwner {
    cancellation: JobCancellation,
}

impl Drop for JobCancellationOwner {
    fn drop(&mut self) {
        let waiters = {
            let mut state = self
                .cancellation
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.async_owners = state.async_owners.saturating_sub(1);
            std::mem::take(&mut state.request_waiters)
        };
        for waiter in waiters {
            waiter.wake();
        }
    }
}
