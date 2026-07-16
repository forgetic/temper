//! Explicit, monotonic per-attempt cancellation handshake.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

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

/// Final state of the attempt's descendant containment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DescendantCleanupStatus {
    Clean,
    Terminated,
    HardKilled,
    Failed(String),
}

impl DescendantCleanupStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Terminated => "terminated",
            Self::HardKilled => "hard_killed",
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

/// The supervisor's single terminal cancellation report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobCleanup {
    pub cancellation: CancellationOutcome,
    pub descendants: DescendantCleanupStatus,
}

#[derive(Debug, Default)]
struct JobCancellationState {
    request_waiters: Vec<Waker>,
    async_owners: usize,
    cleanup: Option<JobCleanup>,
}

/// Attempt-local cancellation handshake shared by the worker shell and every
/// owned effect.
///
/// `CancelJob` and both escalation stages monotonically update `request` and
/// wake the async process owner. The process owner records the real joined
/// cleanup report before returning. Drop remains only an abrupt-owner safety
/// net; the watchdog path does not rely on it.
#[derive(Clone, Debug, Default)]
pub struct JobCancellation {
    request: Arc<AtomicU8>,
    state: Arc<Mutex<JobCancellationState>>,
}

impl JobCancellation {
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
    pub(crate) fn register_async_owner(&self) -> JobCancellationOwner {
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

pub(crate) struct JobCancellationOwner {
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
