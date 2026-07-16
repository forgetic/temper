// SPDX-License-Identifier: MPL-2.0

//! Optional lifecycle checkpoints for deterministic worker restart testing.
//!
//! Production uses the no-op implementation. Cross-component acceptance
//! fixtures install a channel-backed hook so they can stop exactly between a
//! machine transition and its following I/O effect without sleeps or log
//! scraping.

use std::future::Future;
use std::pin::Pin;

/// Stable worker-owned boundaries at which an abrupt process restart can be
/// injected.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WorkerLifecycleCheckpoint {
    /// The machine selected cancellation, before the shell notifies the attempt.
    CancelRequested,
    /// All attempt-owned work quiesced, before the terminal result is recorded.
    Quiesced,
    /// The exact result is durable, before its first delivery attempt.
    ResultRecorded,
    /// Result delivery resolved, before the reply reaches the machine. Losing
    /// the reply here models uncertain delivery.
    ResultDeliveryResolved,
    /// A matching release was accepted, before durable outbox compaction.
    ResultAcknowledged,
    /// The durable terminal record released one local worker permit.
    CapacityReleased,
}

/// Asynchronous test hook installed at worker-owned lifecycle boundaries.
///
/// Implementations must be cancellation-safe: abrupt worker shutdown drops the
/// returned future. `enabled` lets the production no-op preserve the shell's
/// synchronous cancellation fast path.
pub trait WorkerLifecycleHook: Send + Sync + 'static {
    fn enabled(&self) -> bool {
        true
    }

    fn reached(
        &self,
        checkpoint: WorkerLifecycleCheckpoint,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

#[derive(Default)]
pub(crate) struct NoopWorkerLifecycleHook;

impl WorkerLifecycleHook for NoopWorkerLifecycleHook {
    fn enabled(&self) -> bool {
        false
    }

    fn reached(
        &self,
        _checkpoint: WorkerLifecycleCheckpoint,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(std::future::ready(()))
    }
}
