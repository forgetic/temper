// SPDX-License-Identifier: MPL-2.0

//! Ephemeral, head-aware CI terminal-transition monitoring.
//!
//! The runner owns authoritative current-head aggregation. This module only
//! remembers the last successfully observed aggregate long enough to turn a
//! terminal edge into an exact daemon wake hint. The general role poll remains
//! the durable liveness backstop.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use temper_forge::{ChangeHint, ChangeKind, Forge, ItemNumber, RepositoryId};
use temper_runner::{CiStatusObservation, RepositorySet, RepositoryTarget};
use temper_workflow::{CiState, CompiledWorkflow, ValidatedWorkflow};

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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ObservationKey {
    repository: RepositoryId,
    pull_request: ItemNumber,
    head_sha: String,
}

/// In-memory CI aggregate history for configured repositories.
///
/// State is deliberately ephemeral. A successful snapshot replaces all state
/// for its repository, pruning absent pull requests and superseded heads. A
/// failed read never calls [`Self::observe_repository_snapshot`], preserving
/// prior state so recovery cannot repeat an already emitted terminal edge.
#[derive(Debug, Default)]
pub struct CiStatusMonitor {
    observations: BTreeMap<ObservationKey, CiState>,
}

impl CiStatusMonitor {
    /// Creates an empty monitor. A terminal state in the first successful
    /// snapshot is therefore emitted as a transition.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one complete, successful repository snapshot.
    ///
    /// Returned transitions are deterministic by pull-request number. Pending
    /// observations update history but do not emit. A changed terminal verdict
    /// emits even without an intervening observed pending snapshot, allowing a
    /// fast rerun to move directly from failed to passed between polls.
    pub fn observe_repository_snapshot(
        &mut self,
        repository: &RepositoryTarget,
        observations: Vec<CiStatusObservation>,
    ) -> Vec<CiTerminalTransition> {
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

        let mut transitions = Vec::new();
        for observation in current.values() {
            let Some(verdict) = terminal_verdict(observation.state) else {
                continue;
            };
            let key = ObservationKey {
                repository: repository.id.clone(),
                pull_request: observation.pull_request_number,
                head_sha: observation.head_sha.clone(),
            };
            if self.observations.get(&key).copied() == Some(observation.state) {
                continue;
            }
            transitions.push(CiTerminalTransition {
                hint: ChangeHint::pull_request(
                    repository.path.clone(),
                    observation.pull_request_number,
                    ChangeKind::Ci,
                ),
                head_sha: observation.head_sha.clone(),
                verdict,
                completed_at: observation.completed_at,
            });
        }

        self.observations
            .retain(|key, _| key.repository != repository.id || current_keys.contains(key));
        for (pull_request, observation) in current {
            self.observations.insert(
                ObservationKey {
                    repository: repository.id.clone(),
                    pull_request,
                    head_sha: observation.head_sha,
                },
                observation.state,
            );
        }

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
) -> Vec<CiTerminalTransition> {
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

fn terminal_verdict(state: CiState) -> Option<CiTerminalVerdict> {
    match state {
        CiState::Pending => None,
        CiState::Passed => Some(CiTerminalVerdict::Passed),
        CiState::Failed => Some(CiTerminalVerdict::Failed),
    }
}

#[cfg(test)]
mod tests;
