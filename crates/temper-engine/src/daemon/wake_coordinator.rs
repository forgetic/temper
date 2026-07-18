// SPDX-License-Identifier: MPL-2.0

//! Deterministic, bounded ownership of daemon wake scheduling.
//!
//! Wake state is deliberately volatile. A daemon restart loses pending, dirty,
//! and apply-deferred hints; startup broad scans and mandatory poll backstops
//! recover that loss. Webhook delivery is therefore an accelerator, never a
//! correctness dependency.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use temper_engine_io::EngineTime;
use temper_forge::{ChangeHint, ChangeKind, HintArtifactKind, ItemNumber, RepositoryPath};

#[cfg(test)]
pub(crate) use super::wake_scope::MAX_TARGETED_ARTIFACTS;
pub(crate) use super::wake_scope::{
    BroadMode, MergeResult, WakeBatch, WakeLane, WakeScope, WakeTargets, merge_change_kind,
    prioritized_targets,
};

pub(crate) const DEFAULT_MAX_IN_FLIGHT_REPOSITORIES: usize = 2;
pub(crate) const DEFAULT_WAKE_DEBOUNCE: Duration = Duration::from_millis(10);

/// One scheduling submission. An empty `lanes` set means all configured lanes
/// for the repository; explicit lanes are intersected with that configured set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WakeRequest {
    pub(crate) repo: RepositoryPath,
    pub(crate) lanes: BTreeSet<WakeLane>,
    pub(crate) scope: WakeScope,
}

impl WakeRequest {
    pub(crate) fn from_hint(hint: ChangeHint) -> Self {
        let scope = WakeScope::from_hint(&hint);
        Self {
            repo: hint.repo,
            lanes: BTreeSet::new(),
            scope,
        }
    }

    pub(crate) fn broad(repo: RepositoryPath, mode: BroadMode) -> Self {
        Self {
            repo,
            lanes: BTreeSet::new(),
            scope: WakeScope::broad(mode),
        }
    }

    pub(crate) fn broad_for_lanes<I>(repo: RepositoryPath, lanes: I, mode: BroadMode) -> Self
    where
        I: IntoIterator<Item = WakeLane>,
    {
        Self {
            repo,
            lanes: lanes.into_iter().collect(),
            scope: WakeScope::broad(mode),
        }
    }

    pub(crate) fn targeted_for_lanes<I>(
        repo: RepositoryPath,
        lanes: I,
        kind: HintArtifactKind,
        number: ItemNumber,
        change: ChangeKind,
    ) -> Self
    where
        I: IntoIterator<Item = WakeLane>,
    {
        Self {
            repo,
            lanes: lanes.into_iter().collect(),
            scope: WakeScope::targeted(kind, number, change),
        }
    }
}

/// Work admitted under the global repository cap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WakeWork {
    pub(crate) repo: RepositoryPath,
    pub(crate) generation: u64,
    pub(crate) batch: WakeBatch,
    pub(crate) queued_at: EngineTime,
    pub(crate) started_at: EngineTime,
}

impl WakeWork {
    /// Stable correlation key for every event and Forge request in this run.
    pub(crate) fn run_id(&self) -> String {
        format!("{}/{}:{}", self.repo.owner, self.repo.name, self.generation)
    }
}

/// Executor result. Failure never creates work by itself; only an already dirty
/// batch can produce a follow-up generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WakeOutcome {
    Succeeded,
    Failed { reason: String },
}

/// Destination used when final-apply promotion finds a repository still busy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WakePromotion {
    Pending,
    Dirty,
}

/// Structured decisions emitted by the pure coordinator for instrumentation
/// and by the daemon machine for timer/run requests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WakeDecision {
    Accepted {
        repo: RepositoryPath,
        lane: WakeLane,
        scope: WakeScope,
    },
    Coalesced {
        repo: RepositoryPath,
        lane: WakeLane,
        scope: WakeScope,
    },
    Deferred {
        repo: RepositoryPath,
    },
    BroadPromoted {
        repo: RepositoryPath,
        lane: WakeLane,
        mode: BroadMode,
    },
    Promoted {
        repo: RepositoryPath,
        destination: WakePromotion,
    },
    StartTimer {
        repo: RepositoryPath,
        generation: u64,
        delay: Duration,
    },
    Started {
        work: WakeWork,
    },
    DirtyFollowUp {
        repo: RepositoryPath,
        generation: u64,
        lanes: BTreeSet<WakeLane>,
    },
    Finished {
        work: WakeWork,
        outcome: WakeOutcome,
    },
    IgnoredUnknownRepository {
        repo: RepositoryPath,
    },
    IgnoredStaleTimer {
        repo: RepositoryPath,
        generation: u64,
    },
    IgnoredStaleCompletion {
        repo: RepositoryPath,
        generation: u64,
    },
}

/// All volatile wake state for one configured repository.
#[derive(Clone, Debug)]
pub(crate) struct WakeRepositoryState {
    pub(crate) repo: RepositoryPath,
    pub(crate) configured_lanes: BTreeSet<WakeLane>,
    pub(crate) pending: WakeBatch,
    pub(crate) dirty: WakeBatch,
    pub(crate) apply_deferred: WakeBatch,
    timer_generation: Option<u64>,
    ready: bool,
    in_flight_generation: Option<u64>,
    next_generation: u64,
    pending_since: Option<EngineTime>,
    dirty_since: Option<EngineTime>,
    deferred_since: Option<EngineTime>,
}

impl WakeRepositoryState {
    fn new(repo: RepositoryPath, lanes: BTreeSet<WakeLane>) -> Self {
        Self {
            repo,
            configured_lanes: lanes,
            pending: WakeBatch::default(),
            dirty: WakeBatch::default(),
            apply_deferred: WakeBatch::default(),
            timer_generation: None,
            ready: false,
            in_flight_generation: None,
            next_generation: 0,
            pending_since: None,
            dirty_since: None,
            deferred_since: None,
        }
    }

    fn next_generation(&mut self) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1);
        self.next_generation
    }

    pub(crate) fn timer_generation(&self) -> Option<u64> {
        self.timer_generation
    }
}

/// Pure repository scheduler owned exclusively by `DaemonMachine`.
#[derive(Clone, Debug)]
pub(crate) struct WakeCoordinator {
    debounce: Duration,
    max_in_flight_repositories: usize,
    in_flight_repositories: usize,
    repositories: BTreeMap<String, WakeRepositoryState>,
    unresolved_lanes: BTreeSet<WakeLane>,
    configured_repository_limit: usize,
}

impl Default for WakeCoordinator {
    fn default() -> Self {
        Self::new(DEFAULT_WAKE_DEBOUNCE, DEFAULT_MAX_IN_FLIGHT_REPOSITORIES)
    }
}

impl WakeCoordinator {
    pub(crate) fn new(debounce: Duration, max_in_flight_repositories: usize) -> Self {
        Self {
            debounce,
            max_in_flight_repositories,
            in_flight_repositories: 0,
            repositories: BTreeMap::new(),
            unresolved_lanes: BTreeSet::new(),
            configured_repository_limit: 0,
        }
    }

    pub(crate) fn configure(&mut self, debounce: Duration, max_in_flight_repositories: usize) {
        self.debounce = debounce;
        self.max_in_flight_repositories = max_in_flight_repositories;
    }

    pub(crate) fn configure_repository<I>(&mut self, repo: RepositoryPath, lanes: I)
    where
        I: IntoIterator<Item = WakeLane>,
    {
        let lanes = lanes.into_iter().collect::<BTreeSet<_>>();
        let key = repository_key(&repo);
        match self.repositories.get_mut(&key) {
            Some(state) => state.configured_lanes.extend(lanes),
            None => {
                self.repositories
                    .insert(key, WakeRepositoryState::new(repo, lanes));
            }
        }
    }

    pub(crate) fn configure_unresolved_repositories<I>(
        &mut self,
        lanes: I,
        configured_repository_limit: usize,
    ) where
        I: IntoIterator<Item = WakeLane>,
    {
        self.unresolved_lanes.extend(lanes);
        self.configured_repository_limit = self
            .configured_repository_limit
            .max(configured_repository_limit);
    }

    pub(crate) fn configured_repositories(&self) -> Vec<RepositoryPath> {
        self.repositories
            .values()
            .map(|state| state.repo.clone())
            .collect()
    }

    pub(crate) fn repository_state(&self, repo: &RepositoryPath) -> Option<&WakeRepositoryState> {
        self.repositories.get(&repository_key(repo))
    }

    pub(crate) fn in_flight_repositories(&self) -> usize {
        self.in_flight_repositories
    }

    pub(crate) fn pending_target_count(&self, repo: &RepositoryPath) -> usize {
        self.repository_state(repo)
            .map(|state| {
                state.pending.target_count()
                    + state.dirty.target_count()
                    + state.apply_deferred.target_count()
            })
            .unwrap_or(0)
    }

    pub(crate) fn schedule_startup_broad(
        &mut self,
        now: EngineTime,
        repo: RepositoryPath,
        apply_active: bool,
    ) -> Vec<WakeDecision> {
        self.schedule(
            now,
            WakeRequest::broad(repo, BroadMode::Startup),
            apply_active,
        )
    }

    pub(crate) fn schedule(
        &mut self,
        now: EngineTime,
        request: WakeRequest,
        apply_active: bool,
    ) -> Vec<WakeDecision> {
        let key = repository_key(&request.repo);
        if !self.repositories.contains_key(&key)
            && self.repositories.len() < self.configured_repository_limit
            && !self.unresolved_lanes.is_empty()
        {
            self.repositories.insert(
                key.clone(),
                WakeRepositoryState::new(request.repo.clone(), self.unresolved_lanes.clone()),
            );
        }
        let Some(state) = self.repositories.get_mut(&key) else {
            return vec![WakeDecision::IgnoredUnknownRepository { repo: request.repo }];
        };
        let lanes = if request.lanes.is_empty() {
            state.configured_lanes.clone()
        } else {
            request
                .lanes
                .intersection(&state.configured_lanes)
                .cloned()
                .collect()
        };
        if lanes.is_empty() {
            return vec![WakeDecision::IgnoredUnknownRepository { repo: request.repo }];
        }

        let destination = if apply_active {
            BatchDestination::Deferred
        } else if state.in_flight_generation.is_some() {
            BatchDestination::Dirty
        } else {
            BatchDestination::Pending
        };
        let mut decisions = Vec::new();
        for lane in lanes {
            let merge = match destination {
                BatchDestination::Pending => {
                    state.pending_since.get_or_insert(now);
                    state
                        .pending
                        .merge_scope(lane.clone(), request.scope.clone())
                }
                BatchDestination::Dirty => {
                    state.dirty_since.get_or_insert(now);
                    state.dirty.merge_scope(lane.clone(), request.scope.clone())
                }
                BatchDestination::Deferred => {
                    state.deferred_since.get_or_insert(now);
                    state
                        .apply_deferred
                        .merge_scope(lane.clone(), request.scope.clone())
                }
            };
            decisions.push(match merge {
                MergeResult::Accepted => WakeDecision::Accepted {
                    repo: state.repo.clone(),
                    lane,
                    scope: request.scope.clone(),
                },
                MergeResult::Coalesced => WakeDecision::Coalesced {
                    repo: state.repo.clone(),
                    lane,
                    scope: request.scope.clone(),
                },
                MergeResult::BroadPromoted(mode) => WakeDecision::BroadPromoted {
                    repo: state.repo.clone(),
                    lane,
                    mode,
                },
            });
        }

        if apply_active {
            decisions.push(WakeDecision::Deferred {
                repo: state.repo.clone(),
            });
        } else if destination == BatchDestination::Pending
            && state.timer_generation.is_none()
            && !state.ready
        {
            let generation = state.next_generation();
            state.timer_generation = Some(generation);
            decisions.push(WakeDecision::StartTimer {
                repo: state.repo.clone(),
                generation,
                delay: self.debounce,
            });
        }
        decisions
    }

    /// Invalidates every pending timer at the leading edge of an apply window
    /// and moves its batch into apply-deferred state. Already running work is
    /// not cancellable, but no additional repository run can start.
    pub(crate) fn begin_apply(&mut self) -> Vec<WakeDecision> {
        let mut decisions = Vec::new();
        for state in self.repositories.values_mut() {
            if state.pending.is_empty() {
                continue;
            }
            let pending = std::mem::take(&mut state.pending);
            state.apply_deferred.merge_batch(pending);
            state.deferred_since = earliest(state.deferred_since, state.pending_since.take());
            state.timer_generation = None;
            state.ready = false;
            decisions.push(WakeDecision::Deferred {
                repo: state.repo.clone(),
            });
        }
        decisions
    }

    /// Promotes each affected repository exactly once after the final active
    /// apply. Busy repositories receive dirty work; idle repositories receive
    /// one fresh leading-edge timer generation.
    pub(crate) fn promote_apply_deferred(&mut self) -> Vec<WakeDecision> {
        let mut decisions = Vec::new();
        for state in self.repositories.values_mut() {
            if state.apply_deferred.is_empty() {
                continue;
            }
            let deferred = std::mem::take(&mut state.apply_deferred);
            let deferred_since = state.deferred_since.take();
            if state.in_flight_generation.is_some() {
                state.dirty.merge_batch(deferred);
                state.dirty_since = earliest(state.dirty_since, deferred_since);
                decisions.push(WakeDecision::Promoted {
                    repo: state.repo.clone(),
                    destination: WakePromotion::Dirty,
                });
                continue;
            }

            state.pending.merge_batch(deferred);
            state.pending_since = earliest(state.pending_since, deferred_since);
            decisions.push(WakeDecision::Promoted {
                repo: state.repo.clone(),
                destination: WakePromotion::Pending,
            });
            if state.timer_generation.is_none() && !state.ready {
                let generation = state.next_generation();
                state.timer_generation = Some(generation);
                decisions.push(WakeDecision::StartTimer {
                    repo: state.repo.clone(),
                    generation,
                    delay: self.debounce,
                });
            }
        }
        decisions
    }

    pub(crate) fn timer_elapsed(
        &mut self,
        now: EngineTime,
        repo: RepositoryPath,
        generation: u64,
        apply_active: bool,
    ) -> Vec<WakeDecision> {
        let key = repository_key(&repo);
        let Some(state) = self.repositories.get_mut(&key) else {
            return vec![WakeDecision::IgnoredStaleTimer { repo, generation }];
        };
        if state.timer_generation != Some(generation) {
            return vec![WakeDecision::IgnoredStaleTimer { repo, generation }];
        }
        state.timer_generation = None;

        if apply_active {
            let pending = std::mem::take(&mut state.pending);
            state.apply_deferred.merge_batch(pending);
            state.deferred_since = earliest(state.deferred_since, state.pending_since.take());
            state.ready = false;
            return vec![WakeDecision::Deferred {
                repo: state.repo.clone(),
            }];
        }

        state.ready = !state.pending.is_empty();
        self.drain_ready(now)
    }

    pub(crate) fn finish(
        &mut self,
        now: EngineTime,
        work: &WakeWork,
        outcome: WakeOutcome,
        apply_active: bool,
    ) -> Vec<WakeDecision> {
        let key = repository_key(&work.repo);
        let Some(state) = self.repositories.get_mut(&key) else {
            return vec![WakeDecision::IgnoredStaleCompletion {
                repo: work.repo.clone(),
                generation: work.generation,
            }];
        };
        if state.in_flight_generation != Some(work.generation) {
            return vec![WakeDecision::IgnoredStaleCompletion {
                repo: work.repo.clone(),
                generation: work.generation,
            }];
        }

        state.in_flight_generation = None;
        self.in_flight_repositories = self.in_flight_repositories.saturating_sub(1);
        let mut decisions = vec![WakeDecision::Finished {
            work: work.clone(),
            outcome,
        }];

        if !state.dirty.is_empty() {
            let dirty = std::mem::take(&mut state.dirty);
            let dirty_since = state.dirty_since.take();
            if apply_active {
                state.apply_deferred.merge_batch(dirty);
                state.deferred_since = earliest(state.deferred_since, dirty_since);
                decisions.push(WakeDecision::Deferred {
                    repo: state.repo.clone(),
                });
            } else {
                let lanes = dirty.lanes().keys().cloned().collect();
                state.pending.merge_batch(dirty);
                state.pending_since = earliest(state.pending_since, dirty_since);
                let generation = state.next_generation();
                state.timer_generation = Some(generation);
                decisions.push(WakeDecision::DirtyFollowUp {
                    repo: state.repo.clone(),
                    generation,
                    lanes,
                });
                decisions.push(WakeDecision::StartTimer {
                    repo: state.repo.clone(),
                    generation,
                    delay: self.debounce,
                });
            }
        }

        if !apply_active {
            decisions.extend(self.drain_ready(now));
        }
        decisions
    }

    fn drain_ready(&mut self, now: EngineTime) -> Vec<WakeDecision> {
        let mut decisions = Vec::new();
        if self.max_in_flight_repositories == 0 {
            return decisions;
        }
        let keys = self.repositories.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            if self.in_flight_repositories >= self.max_in_flight_repositories {
                break;
            }
            let state = self
                .repositories
                .get_mut(&key)
                .expect("repository key came from map");
            if !state.ready || state.in_flight_generation.is_some() || state.pending.is_empty() {
                continue;
            }
            let generation = state.next_generation;
            let batch = std::mem::take(&mut state.pending);
            let queued_at = state.pending_since.take().unwrap_or(now);
            state.ready = false;
            state.in_flight_generation = Some(generation);
            self.in_flight_repositories += 1;
            decisions.push(WakeDecision::Started {
                work: WakeWork {
                    repo: state.repo.clone(),
                    generation,
                    batch,
                    queued_at,
                    started_at: now,
                },
            });
        }
        decisions
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BatchDestination {
    Pending,
    Dirty,
    Deferred,
}

fn repository_key(repo: &RepositoryPath) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

fn earliest(left: Option<EngineTime>, right: Option<EngineTime>) -> Option<EngineTime> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
#[path = "wake_coordinator_scope_tests.rs"]
mod scope_tests;

#[cfg(test)]
#[path = "wake_coordinator_tests.rs"]
mod tests;
