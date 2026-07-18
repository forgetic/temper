// SPDX-License-Identifier: MPL-2.0

//! Worker-protocol + webhook transport for one daemon process.
//!
//! [`Daemon`] is a cloneable handle that submits `<io-event-completion>`s to the
//! daemon's engine loop. The deterministic logic — protocol handling, long-poll
//! waiters, apply-window bookkeeping, webhook verification — lives in
//! [`machine::DaemonMachine`], a pure state machine; all I/O (HTTP responses,
//! timers, result application, wake scans) is performed by
//! [`executor::DaemonExecutor`] on the engine runtime.

mod activity_transport;
mod change_source_wiring;
mod context_reader;
mod context_transport;
mod executor;
mod handle;
mod handlers;
mod machine;
mod protocol;
mod result_application;
mod shutdown;
pub mod state_dto;
// The coordinator's complete role/mechanical/poll contract is consumed in
// stages; constructors not used by the compatibility scanner remain exercised
// by its pure tests until the targeted executor lands.
#[allow(dead_code)]
mod wake_coordinator;
mod wake_observability;
#[allow(dead_code)]
mod wake_scope;
mod webhook_handlers;
mod webhook_wiring;

use std::sync::Arc;

use temper_engine_io::CqSender;
use temper_forge::{ChangeKind, RepositoryPath};
use temper_runner::ArtifactAddress;

use change_source_wiring::ChangeSourceListener;
use machine::DaemonCompletion;
use wake_coordinator::{WakeOutcome, WakeWork};

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
    wake_executor_slot: Arc<std::sync::Mutex<Option<Arc<dyn WakeExecutor>>>>,
    context_reader_slot: Arc<std::sync::Mutex<Option<Arc<dyn context_reader::ContextReader>>>>,
    trace_query_slot: Arc<std::sync::Mutex<Option<crate::trace_query::TraceQueryService>>>,
    trace_journal_slot: Arc<std::sync::Mutex<Option<crate::AgentTraceJournal>>>,
    change_source_listeners: Arc<std::sync::Mutex<Vec<ChangeSourceListener>>>,
    artifact_catalog: Arc<crate::ConfiguredRepositoryCatalog>,
    pub(crate) artifact_context: Option<Arc<crate::ArtifactContextBundleService>>,
}

/// Type-erased execution boundary for work admitted by the daemon-owned wake
/// coordinator.
pub(crate) trait WakeExecutor: Send + Sync {
    fn run(
        &self,
        work: WakeWork,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = WakeOutcome> + Send>>;
}

/// Type-erased execution boundary for mechanical work already admitted by the
/// daemon coordinator. Both methods are required so implementations cannot
/// fall back to an uncoordinated or lossy hinted path. A successful result
/// reports whether the pass mutated workflow state, allowing the coordinator to
/// wake subscribed roles even when the provider drops the mutation webhook.
pub trait CoordinatedMechanical: Send + Sync {
    /// Executes repository-wide mechanical reconciliation and reports whether
    /// it changed workflow state.
    fn run_coordinated_broad(
        &self,
        repo: RepositoryPath,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send>>;

    /// Executes one exact artifact request and reports whether it changed
    /// workflow state.
    fn run_coordinated_targeted(
        &self,
        repo: RepositoryPath,
        artifact: ArtifactAddress,
        change: ChangeKind,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send>>;
}

impl<F: temper_forge::Forge + Send + Sync + ?Sized + 'static> CoordinatedMechanical
    for crate::MechanicalTrigger<F>
{
    fn run_coordinated_broad(
        &self,
        repo: RepositoryPath,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send>> {
        let trigger = self.clone();
        Box::pin(async move {
            trigger
                .run_coordinated(crate::MechanicalScope::Hinted(vec![
                    temper_forge::ChangeHint::repository(repo, ChangeKind::Unknown),
                ]))
                .await
                .map(|progress| progress.changed)
                .map_err(|error| error.to_string())
        })
    }

    fn run_coordinated_targeted(
        &self,
        repo: RepositoryPath,
        artifact: ArtifactAddress,
        change: ChangeKind,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send>> {
        let trigger = self.clone();
        Box::pin(async move {
            trigger
                .run_coordinated(crate::MechanicalScope::Targeted(vec![(
                    repo, artifact, change,
                )]))
                .await
                .map(|progress| progress.changed)
                .map_err(|error| error.to_string())
        })
    }
}
