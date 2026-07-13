// SPDX-License-Identifier: MPL-2.0

//! Worker-protocol + webhook transport for one daemon process.
//!
//! [`Daemon`] is a cloneable handle that submits `<io-event-completion>`s to the
//! daemon's engine loop. The deterministic logic — protocol handling, long-poll
//! waiters, apply-window bookkeeping, webhook verification — lives in
//! [`machine::DaemonMachine`], a pure state machine; all I/O (HTTP responses,
//! timers, result application, wake scans) is performed by
//! [`executor::DaemonExecutor`] on the engine runtime.

mod change_source_wiring;
mod context_reader;
mod context_transport;
mod executor;
mod handle;
mod handlers;
mod machine;
mod protocol;
pub mod state_dto;
// The coordinator's complete role/mechanical/poll contract is consumed in
// stages; constructors not used by the compatibility scanner remain exercised
// by its pure tests until the targeted executor lands.
#[allow(dead_code)]
mod wake_coordinator;
mod webhook_handlers;
mod webhook_wiring;

use std::sync::Arc;

use temper_engine_io::CqSender;

use change_source_wiring::ChangeSourceListener;
use machine::DaemonCompletion;

pub use handle::{h1_handler, serve};
pub use state_dto::{
    ArtifactDto, DaemonStateSnapshot, JobDto, RoleSaturationDto, WorkerCapabilityDto, WorkerDto,
    WorkersDto,
};

/// Worker-protocol + webhook transport handle for one daemon process. See the
/// module docs for the machine/executor split.
#[derive(Clone)]
pub struct Daemon {
    cq: CqSender<DaemonCompletion>,
    scanner_slot: Arc<std::sync::Mutex<Option<Arc<dyn WakeScanner>>>>,
    context_reader_slot: Arc<std::sync::Mutex<Option<Arc<dyn context_reader::ContextReader>>>>,
    change_source_listeners: Arc<std::sync::Mutex<Vec<ChangeSourceListener>>>,
    artifact_catalog: Arc<crate::ConfiguredRepositoryCatalog>,
    pub(crate) artifact_context: Option<Arc<crate::ArtifactContextBundleService>>,
}

/// Type-erased wake scanner installed by webhook or change-source wiring.
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

impl<F: temper_forge::Forge + Send + Sync + ?Sized + 'static> HintedMechanical
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
