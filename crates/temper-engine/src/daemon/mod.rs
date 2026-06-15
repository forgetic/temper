// SPDX-License-Identifier: MPL-2.0

//! Worker-protocol + webhook transport for one daemon process.
//!
//! [`Daemon`] is a cloneable handle that submits `<io-event-completion>`s to the
//! daemon's engine loop. The deterministic logic — protocol handling, long-poll
//! waiters, apply-window bookkeeping, webhook verification — lives in
//! [`machine::DaemonMachine`], a pure state machine; all I/O (HTTP responses,
//! timers, result application, wake scans) is performed by
//! [`executor::DaemonExecutor`] on the engine runtime.

mod executor;
mod handle;
mod handlers;
mod machine;
mod protocol;
mod webhook_wiring;

use std::sync::Arc;

use temper_engine_io::CqSender;

use machine::DaemonCompletion;

pub use handle::{h1_handler, serve};

/// Worker-protocol + webhook transport handle for one daemon process. See the
/// module docs for the machine/executor split.
#[derive(Clone)]
pub struct Daemon {
    cq: CqSender<DaemonCompletion>,
    scanner_slot: Arc<std::sync::Mutex<Option<Arc<dyn WakeScanner>>>>,
}

/// Type-erased webhook wake scanner installed by [`Daemon::with_webhook`].
pub(crate) trait WakeScanner: Send + Sync {
    fn scan(
        &self,
        hint: temper_runner::ChangeHint,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
}

/// Type-erased mechanical accelerator: run a hinted mechanical pass for the
/// repositories named by a webhook delivery. Lets the webhook path drive the
/// generic [`crate::MechanicalTrigger<F>`] without the daemon being generic over
/// `F`.
pub trait HintedMechanical: Send + Sync {
    /// Run a mechanical pass covering only the hinted repositories.
    fn run_hinted(
        &self,
        hints: Vec<temper_forge::ChangeHint>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
}

impl<F: temper_forge::Forge + Send + Sync + 'static> HintedMechanical
    for crate::MechanicalTrigger<F>
{
    fn run_hinted(
        &self,
        hints: Vec<temper_forge::ChangeHint>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let trigger = self.clone();
        Box::pin(async move {
            trigger.run_hinted(hints).await;
        })
    }
}
