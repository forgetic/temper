// SPDX-License-Identifier: MPL-2.0

//! Ephemeral, head-aware CI transition monitoring.
//!
//! The runner owns authoritative current-head aggregation and distinguishes a
//! genuinely missing current-head job set from active pending work. This module
//! remembers successful observations long enough to turn terminal edges and an
//! uninterrupted, grace-aged missing interval into exact daemon wake hints. The
//! general role poll remains the durable liveness backstop.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_engine_io::Spawner;
use temper_forge::{ChangeHint, ChangeKind, Forge, ItemNumber, RepositoryId};
use temper_runner::{CiStatusObservation, RepositorySet, RepositoryTarget};
use temper_workflow::{CiState, CompiledWorkflow, ValidatedWorkflow};

use crate::lease_applier::WallClock;

/// Terminal CI verdict carried beside a synthetic CI change hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CiTerminalVerdict {
    /// Every latest current-head job completed successfully.
    Passed,
    /// Every latest current-head job completed and at least one did not succeed.
    Failed,
}

/// One newly observed terminal aggregate for an exact pull request and head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiTerminalTransition {
    /// Exact pull-request-scoped `ChangeKind::Ci` wake hint.
    pub hint: ChangeHint,
    /// Current head SHA whose aggregate became terminal.
    pub head_sha: String,
    /// Newly observed terminal verdict.
    pub verdict: CiTerminalVerdict,
    /// Latest-job-set completion time, when every latest job supplied one.
    pub completed_at: Option<DateTime<Utc>>,
}

/// One uninterrupted missing-current-head interval whose grace has expired.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiMissingCurrentHeadTransition {
    /// Exact pull-request-scoped `ChangeKind::Ci` wake hint.
    pub hint: ChangeHint,
    /// Exact current head SHA for which no matching job was observed.
    pub head_sha: String,
    /// Wall-clock time of the first successful missing observation.
    pub first_observed_at: DateTime<Utc>,
}

/// A transition emitted by the narrow CI status monitor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CiStatusTransition {
    /// A current-head aggregate newly became terminal.
    Terminal(CiTerminalTransition),
    /// Current-head jobs remained missing for the configured grace period.
    MissingCurrentHead(CiMissingCurrentHeadTransition),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ObservationKey {
    repository: RepositoryId,
    pull_request: ItemNumber,
    head_sha: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RecordedObservation {
    Present(CiState),
    Missing { first_observed_at: DateTime<Utc> },
}

/// In-memory CI aggregate and missing-interval history for configured repositories.
///
/// State is deliberately ephemeral. A successful snapshot replaces all state
/// for its repository, pruning absent pull requests and superseded heads. A
/// failed read never calls [`Self::observe_repository_snapshot`], preserving
/// prior state without advancing or emitting a missing-current-head recovery.
pub struct CiStatusMonitor {
    observations: BTreeMap<ObservationKey, RecordedObservation>,
    missing_grace: Duration,
    clock: WallClock,
}

impl CiStatusMonitor {
    /// Creates an empty monitor with an injected grace and wall clock.
    ///
    /// A terminal state in the first successful snapshot is emitted as a
    /// transition. Missing CI instead starts a fresh grace window.
    pub fn new(missing_grace: Duration, clock: WallClock) -> Self {
        Self {
            observations: BTreeMap::new(),
            missing_grace,
            clock,
        }
    }

    /// Applies one complete, successful repository snapshot.
    ///
    /// Returned transitions are deterministic by pull-request number. Present
    /// pending observations update history but do not emit. A changed terminal
    /// verdict emits even without an intervening observed pending snapshot. A
    /// missing observation emits whenever its uninterrupted grace has elapsed.
    /// Re-emission is deliberate: submissions still pass through bounded wake
    /// coalescing, while transient validation or mutation failures receive a
    /// later pass until authoritative observations change or parking completes.
    pub fn observe_repository_snapshot(
        &mut self,
        repository: &RepositoryTarget,
        observations: Vec<CiStatusObservation>,
    ) -> Vec<CiStatusTransition> {
        // The runner emits one current head per PR. Keying by PR here also
        // makes a malformed duplicate deterministic (the final observation is
        // authoritative) and prevents one snapshot retaining two heads.
        let current: BTreeMap<ItemNumber, CiStatusObservation> = observations
            .into_iter()
            .map(|observation| (observation.pull_request_number, observation))
            .collect();
        let current_keys: BTreeSet<ObservationKey> = current
            .values()
            .map(|observation| ObservationKey {
                repository: repository.id.clone(),
                pull_request: observation.pull_request_number,
                head_sha: observation.head_sha.clone(),
            })
            .collect();
        // Read the injected clock only for a successful repository snapshot so
        // failed Forge reads cannot age or emit recovery from stale evidence.
        let now = (self.clock)();

        let mut transitions = Vec::new();
        let mut next = Vec::with_capacity(current.len());
        for observation in current.values() {
            let key = ObservationKey {
                repository: repository.id.clone(),
                pull_request: observation.pull_request_number,
                head_sha: observation.head_sha.clone(),
            };
            let prior = self.observations.get(&key).cloned();
            let recorded = if observation.current_head_jobs_present {
                if prior != Some(RecordedObservation::Present(observation.state)) {
                    if let Some(verdict) = terminal_verdict(observation.state) {
                        transitions.push(CiStatusTransition::Terminal(CiTerminalTransition {
                            hint: ci_hint(repository, observation.pull_request_number),
                            head_sha: observation.head_sha.clone(),
                            verdict,
                            completed_at: observation.completed_at,
                        }));
                    }
                }
                RecordedObservation::Present(observation.state)
            } else {
                let first_observed_at = match prior {
                    Some(RecordedObservation::Missing { first_observed_at }) => first_observed_at,
                    _ => {
                        tracing::info!(
                            target: "temper::engine",
                            service = "engine",
                            repo = %repository.display_path(),
                            repository_id = %repository.id,
                            pull_request = observation.pull_request_number.get(),
                            head_sha = %observation.head_sha,
                            first_observed_at = %now,
                            grace_secs = self.missing_grace.as_secs(),
                            "CI status monitor first observed missing current-head jobs"
                        );
                        now
                    }
                };

                if now
                    .signed_duration_since(first_observed_at)
                    .to_std()
                    .is_ok_and(|elapsed| elapsed >= self.missing_grace)
                {
                    transitions.push(CiStatusTransition::MissingCurrentHead(
                        CiMissingCurrentHeadTransition {
                            hint: ci_hint(repository, observation.pull_request_number),
                            head_sha: observation.head_sha.clone(),
                            first_observed_at,
                        },
                    ));
                    tracing::warn!(
                        target: "temper::engine",
                        service = "engine",
                        repo = %repository.display_path(),
                        repository_id = %repository.id,
                        pull_request = observation.pull_request_number.get(),
                        head_sha = %observation.head_sha,
                        first_observed_at = %first_observed_at,
                        grace_expired_at = %now,
                        grace_secs = self.missing_grace.as_secs(),
                        "CI status monitor missing current-head grace expired"
                    );
                }

                RecordedObservation::Missing { first_observed_at }
            };
            next.push((key, recorded));
        }

        self.observations
            .retain(|key, _| key.repository != repository.id || current_keys.contains(key));
        self.observations.extend(next);

        transitions
    }
}

/// Reads and applies one narrow CI snapshot for every configured repository.
///
/// Repository failures are logged and isolated: other repositories still run,
/// and failed repositories retain their prior monitor state. The returned
/// transitions are ordered by configured repository and then PR number.
pub async fn run_ci_status_monitor_tick<F: Forge + ?Sized>(
    monitor: &mut CiStatusMonitor,
    forge: &F,
    repositories: &RepositorySet,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
) -> Vec<CiStatusTransition> {
    let mut transitions = Vec::new();
    for repository in repositories.repositories() {
        match temper_runner::read_ci_status_observations(forge, &repository.id, workflow, compiled)
            .await
        {
            Ok(observations) => {
                transitions.extend(monitor.observe_repository_snapshot(repository, observations))
            }
            Err(error) => tracing::warn!(
                target: "temper::engine",
                service = "engine",
                repo = %repository.display_path(),
                repository_id = %repository.id,
                %error,
                "CI status monitor repository read failed"
            ),
        }
    }
    transitions
}

/// Spawns the fixed-delay CI monitor. Each terminal or grace-expired missing
/// edge is submitted as an exact daemon wake; the cadence task performs no
/// mechanical or role work itself and therefore cannot bypass bounded
/// coordinator admission.
#[allow(clippy::too_many_arguments)]
pub fn spawn_ci_status_monitor<F: Forge + Send + Sync + ?Sized + 'static>(
    spawner: &Arc<dyn Spawner>,
    daemon: crate::Daemon,
    forge: Arc<F>,
    repositories: RepositorySet,
    workflow: Arc<ValidatedWorkflow>,
    compiled: Arc<CompiledWorkflow>,
    cadence: Duration,
    missing_grace: Duration,
    clock: WallClock,
) {
    // Cadence ticks are fixed-delay and never overlap. Taking the monitor out of
    // the mutex before awaiting keeps the spawned future Send without holding a
    // synchronous guard across Forge I/O.
    let monitor = Arc::new(Mutex::new(Some(CiStatusMonitor::new(missing_grace, clock))));
    temper_engine_io::spawn_cadence_loop(spawner, cadence, move || {
        let daemon = daemon.clone();
        let forge = Arc::clone(&forge);
        let repositories = repositories.clone();
        let workflow = Arc::clone(&workflow);
        let compiled = Arc::clone(&compiled);
        let monitor = Arc::clone(&monitor);
        async move {
            let mut state = monitor
                .lock()
                .expect("CI status monitor state")
                .take()
                .expect("CI status monitor ticks do not overlap");
            let transitions = run_ci_status_monitor_tick(
                &mut state,
                forge.as_ref(),
                &repositories,
                workflow.as_ref(),
                compiled.as_ref(),
            )
            .await;
            *monitor.lock().expect("CI status monitor state") = Some(state);
            for transition in transitions {
                daemon.submit_ci_poll_transition(transition);
            }
        }
    });
}

fn ci_hint(repository: &RepositoryTarget, pull_request: ItemNumber) -> ChangeHint {
    ChangeHint::pull_request(repository.path.clone(), pull_request, ChangeKind::Ci)
}

fn terminal_verdict(state: CiState) -> Option<CiTerminalVerdict> {
    match state {
        CiState::Pending => None,
        CiState::Passed => Some(CiTerminalVerdict::Passed),
        CiState::Failed => Some(CiTerminalVerdict::Failed),
    }
}

#[cfg(test)]
mod tests;
