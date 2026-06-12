// SPDX-License-Identifier: MPL-2.0

//! Mechanical backstop loop owned by the daemon process.

use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_forge::{Forge, ForgeError};
use temper_runner::{
    MultiRepoMechanicalWorker, Progress, RepositoryJournal, RepositorySet, Worker, WorkerError,
};
use temper_workflow::{InMemoryJournal, LeasePolicy, ValidatedWorkflow};

/// Configuration for the daemon's mechanical backstop loop.
#[derive(Clone, Debug)]
pub struct MechanicalBackstopConfig {
    /// Repositories ticked in order on each pass.
    pub repositories: RepositorySet,
    /// Delay after one complete pass before the next pass starts.
    pub cadence: Duration,
    /// Lease policy for mechanical transitions (reuse the daemon's lease TTL).
    pub lease_policy: LeasePolicy,
}

/// Runs one mechanical pass over the configured repositories.
///
/// The worker is constructed per call so tests can tick deterministically while
/// the caller owns the per-repository journals across calls. Workspace-backed
/// automations intentionally run with no external tool executors bound; those
/// automations no-op safely until a binding exists.
pub async fn run_mechanical_backstop_tick<F: Forge + ?Sized>(
    forge: &F,
    workflow: &ValidatedWorkflow,
    now: DateTime<Utc>,
    config: &MechanicalBackstopConfig,
    journals: &[InMemoryJournal],
) -> Result<Progress, WorkerError> {
    if journals.len() != config.repositories.repositories().len() {
        let error = setup_error(format!(
            "mechanical backstop has {} repositories but {} journals",
            config.repositories.repositories().len(),
            journals.len()
        ));
        eprintln!("temper-daemon: mechanical backstop tick failed: {error}");
        return Err(error);
    }

    let journal_bindings: Vec<RepositoryJournal<'_, InMemoryJournal>> = config
        .repositories
        .repositories()
        .iter()
        .zip(journals.iter())
        .map(|(repository, journal)| RepositoryJournal {
            repository: &repository.id,
            journal,
        })
        .collect();
    let worker = match MultiRepoMechanicalWorker::new(
        workflow,
        forge,
        config.repositories.clone(),
        journal_bindings,
        config.lease_policy,
    ) {
        Ok(worker) => worker,
        Err(error) => {
            let error = setup_error(format!("mechanical backstop setup failed: {error}"));
            eprintln!("temper-daemon: mechanical backstop tick failed: {error}");
            return Err(error);
        }
    };

    match worker.tick(now).await {
        Ok(progress) => Ok(progress),
        Err(error) => {
            eprintln!("temper-daemon: mechanical backstop tick failed: {error}");
            Err(error)
        }
    }
}

/// Owns the per-repo journals and runs the mechanical backstop until the
/// runtime shuts down, as a machine-driven cadence loop on the engine.
///
/// Tick errors are logged by [`run_mechanical_backstop_tick`] and skipped so a
/// transient backend failure does not stop the daemon-owned backstop.
pub fn spawn_mechanical_backstop<F: Forge + Send + Sync + 'static>(
    forge: std::sync::Arc<F>,
    workflow: std::sync::Arc<ValidatedWorkflow>,
    config: MechanicalBackstopConfig,
) {
    let handle = skein::runtime::Runtime::current_handle()
        .expect("mechanical backstop requires a running engine runtime");
    let journals: std::sync::Arc<Vec<InMemoryJournal>> = std::sync::Arc::new(
        config
            .repositories
            .repositories()
            .iter()
            .map(|_| InMemoryJournal::new())
            .collect(),
    );
    let cadence = config.cadence;
    temper_io_engine::spawn_cadence_loop(&handle, cadence, move || {
        let forge = std::sync::Arc::clone(&forge);
        let workflow = std::sync::Arc::clone(&workflow);
        let journals = std::sync::Arc::clone(&journals);
        let config = config.clone();
        async move {
            let _ = run_mechanical_backstop_tick(
                forge.as_ref(),
                workflow.as_ref(),
                Utc::now(),
                &config,
                &journals,
            )
            .await;
        }
    });
}

fn setup_error(message: String) -> WorkerError {
    WorkerError::Forge(ForgeError::Backend(message))
}
