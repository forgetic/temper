//! Runtime-supplied gate signals for the planner.
//!
//! Some gate conditions ask about facts the pure planner must not derive
//! itself: whether prerequisite work has landed (`dependencies_resolved`),
//! whether native CI passed or failed (`ci_passed`/`ci_failed`), and whether
//! native reviews approve or request changes. Those facts are read fresh from
//! the Forge by the runtime and supplied to the planner as a small signal
//! bundle.
//!
//! [`GateSignals`] bundles every runtime signal a transition plan may consult.
//! Bundling (rather than threading one parameter per gate) keeps adding signals
//! from re-threading every planner call site, and gives one obvious place to
//! construct "the facts the runtime read before planning". The [`Planner`] only
//! reads the bundle; it never lists jobs, reviews, or talks to a Forge.
//!
//! [`Planner`]: super::Planner

use super::{DependencyStatus, queue::QueueQuery};
use crate::ids::TransitionId;
use crate::validated::{GateCondition, ValidatedTransition, ValidatedWorkflow};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use temper_forge::{CiJob, CiJobConclusion, CiJobStatus, PullRequestReviewStatus};

/// Runtime signal categories a queue or transition may need.
///
/// The planner can evaluate label and state conditions from a classified
/// artifact alone. Dependency, CI, and review conditions require fresh Forge
/// reads, so scanners and executors use this interest set to load only the
/// signal families a candidate queue or transition can actually inspect.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SignalNeeds {
    /// Whether dependency target state is needed.
    pub dependencies: bool,
    /// Whether native CI jobs are needed.
    pub ci: bool,
    /// Whether native pull-request reviews are needed.
    pub review: bool,
}

impl SignalNeeds {
    /// Builds an explicit interest set.
    pub const fn new(dependencies: bool, ci: bool, review: bool) -> Self {
        Self {
            dependencies,
            ci,
            review,
        }
    }

    /// No runtime signal families are needed.
    pub const fn none() -> Self {
        Self::new(false, false, false)
    }

    /// Every runtime signal family is needed.
    pub const fn all() -> Self {
        Self::new(true, true, true)
    }

    /// Returns whether no runtime signal families are requested.
    pub const fn is_empty(self) -> bool {
        !self.dependencies && !self.ci && !self.review
    }

    /// Returns the union of two interest sets.
    pub const fn union(self, other: Self) -> Self {
        Self::new(
            self.dependencies || other.dependencies,
            self.ci || other.ci,
            self.review || other.review,
        )
    }

    /// Returns the runtime signals needed to evaluate a gate/queue condition.
    pub const fn for_condition(condition: &GateCondition) -> Self {
        match condition {
            GateCondition::DependenciesResolved => Self::new(true, false, false),
            GateCondition::CiPassed | GateCondition::CiFailed => Self::new(false, true, false),
            GateCondition::ReviewApproved | GateCondition::ReviewChangesRequested => {
                Self::new(false, false, true)
            }
            GateCondition::LabelPresent(_) | GateCondition::StateEquals { .. } => Self::none(),
        }
    }

    /// Returns the runtime signals needed to evaluate a queue's condition.
    pub fn for_queue<Q: QueueQuery + ?Sized>(queue: &Q) -> Self {
        queue
            .queue_condition()
            .map_or_else(Self::none, Self::for_condition)
    }

    /// Returns the runtime signals needed by a transition's required gates.
    pub fn for_transition(workflow: &ValidatedWorkflow, transition: &ValidatedTransition) -> Self {
        transition
            .requires_gates
            .iter()
            .filter_map(|gate_id| workflow.gates().iter().find(|gate| &gate.id == gate_id))
            .filter_map(|gate| gate.condition.as_ref())
            .fold(Self::none(), |needs, condition| {
                needs.union(Self::for_condition(condition))
            })
    }

    /// Returns the runtime signals needed by a transition id, or none when the
    /// workflow has no such transition.
    pub fn for_transition_id(workflow: &ValidatedWorkflow, transition: &TransitionId) -> Self {
        workflow
            .transitions()
            .iter()
            .find(|candidate| &candidate.id == transition)
            .map_or_else(Self::none, |candidate| {
                Self::for_transition(workflow, candidate)
            })
    }
}

impl ValidatedWorkflow {
    /// Returns the runtime signals needed by a transition's required gates.
    pub fn signal_needs_for_transition(&self, transition: &TransitionId) -> SignalNeeds {
        SignalNeeds::for_transition_id(self, transition)
    }
}

/// Runtime-supplied aggregate state for an artifact's native CI.
///
/// `ci_passed` asks whether the artifact's CI is green; `ci_failed` asks
/// whether the current CI run settled red. Like the dependency signal, the
/// verdict is decided by the runtime — never derived inside the pure planner —
/// and supplied here as a small value the planner only reads. Build it from
/// fresh [`CiJob`]s with [`CiStatus::from_jobs_for_head`] when a current head is
/// known, or [`CiStatus::from_jobs`] when the provider supplies no head; these
/// are the documented places where the aggregation rule lives.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CiStatus {
    state: CiState,
    completed_at: Option<DateTime<Utc>>,
}

/// Portable aggregate state for the latest native CI jobs on an artifact.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CiState {
    /// CI has not run yet, is still running, or has no terminal aggregate.
    #[default]
    Pending,
    /// The latest job per name all completed successfully.
    Passed,
    /// The latest job per name all completed and at least one was non-success.
    Failed,
}

impl CiStatus {
    /// A status whose CI is pending/unknown (the default, conservative verdict).
    pub fn new() -> Self {
        Self::default()
    }

    /// A status whose CI has passed.
    pub fn passed() -> Self {
        Self {
            state: CiState::Passed,
            completed_at: None,
        }
    }

    /// A status whose CI has failed.
    pub fn failed() -> Self {
        Self {
            state: CiState::Failed,
            completed_at: None,
        }
    }

    /// Builds a status directly from a pass verdict.
    pub fn with_passed(passed: bool) -> Self {
        if passed { Self::passed() } else { Self::new() }
    }

    /// Builds a status directly from an aggregate CI state.
    pub fn with_state(state: CiState) -> Self {
        Self {
            state,
            completed_at: None,
        }
    }

    /// Returns the aggregate CI state.
    pub fn state(&self) -> CiState {
        self.state
    }

    /// Returns when the complete latest-job set became terminal, when known.
    ///
    /// Pending aggregates never expose a completion time. A terminal aggregate
    /// exposes the maximum completion timestamp only when every latest job
    /// supplies one, so the value cannot predate an un-timestamped job.
    pub fn completed_at(&self) -> Option<DateTime<Utc>> {
        self.completed_at
    }

    /// Returns whether CI has passed.
    pub fn is_passed(&self) -> bool {
        self.state == CiState::Passed
    }

    /// Returns whether CI has failed.
    pub fn is_failed(&self) -> bool {
        self.state == CiState::Failed
    }

    /// Computes the CI verdict from a set of CI jobs.
    ///
    /// The jobs are reduced to the latest job per name (by `created_at`). CI is
    /// *passed* when that set is non-empty and every latest-per-name job has
    /// status [`CiJobStatus::Completed`] with conclusion
    /// [`CiJobConclusion::Success`]. CI is *failed* when that set is non-empty,
    /// every latest-per-name job is completed, and at least one latest job has a
    /// non-success conclusion. Otherwise CI is pending/unknown: no jobs, queued
    /// jobs, or running jobs neither pass nor fail. See ADR 0014 and ADR 0017.
    pub fn from_jobs(jobs: &[CiJob]) -> Self {
        Self::from_job_iter(jobs.iter())
    }

    /// Computes the CI verdict from jobs that independently identify `head_sha`.
    ///
    /// A non-empty head filters jobs before latest-per-name aggregation, so a
    /// provider returning results for another commit cannot satisfy the gate.
    /// Exact SHA matches are accepted at any length. Case-insensitive prefix
    /// matches are accepted only when the shorter SHA has at least seven
    /// characters, allowing normal full/abbreviated SHA pairs without treating
    /// unsafe short prefixes as evidence. Empty job SHAs never identify a head.
    /// If `head_sha` is absent or empty, this preserves [`Self::from_jobs`]
    /// behavior for providers that cannot supply a head.
    pub fn from_jobs_for_head(jobs: &[CiJob], head_sha: Option<&str>) -> Self {
        let Some(head_sha) = head_sha.map(str::trim).filter(|sha| !sha.is_empty()) else {
            return Self::from_jobs(jobs);
        };

        Self::from_job_iter(
            jobs.iter()
                .filter(|job| sha_identifies_head(&job.commit_sha, head_sha)),
        )
    }

    fn from_job_iter<'a>(jobs: impl IntoIterator<Item = &'a CiJob>) -> Self {
        let mut latest: HashMap<&'a str, &'a CiJob> = HashMap::new();
        for job in jobs {
            latest
                .entry(job.name.as_str())
                .and_modify(|current| {
                    if job.created_at >= current.created_at {
                        *current = job;
                    }
                })
                .or_insert(job);
        }
        if latest.is_empty()
            || latest
                .values()
                .any(|job| job.status != CiJobStatus::Completed)
        {
            return Self::new();
        }
        let state = if latest
            .values()
            .all(|job| job.conclusion == Some(CiJobConclusion::Success))
        {
            CiState::Passed
        } else {
            CiState::Failed
        };
        let completed_at = latest
            .values()
            .map(|job| job.completed_at)
            .collect::<Option<Vec<_>>>()
            .and_then(|timestamps| timestamps.into_iter().max());
        Self {
            state,
            completed_at,
        }
    }
}

fn sha_identifies_head(job_sha: &str, head_sha: &str) -> bool {
    let job_sha = job_sha.trim();
    if job_sha.is_empty() {
        return false;
    }
    if job_sha.eq_ignore_ascii_case(head_sha) {
        return true;
    }

    let (shorter, longer) = if job_sha.len() < head_sha.len() {
        (job_sha, head_sha)
    } else {
        (head_sha, job_sha)
    };
    shorter.len() >= 7
        && longer
            .get(..shorter.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(shorter))
}

/// Runtime-supplied verdict for a pull request's native review state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReviewStatus {
    approved: bool,
    changes_requested: bool,
}

impl ReviewStatus {
    /// Builds a review status from explicit aggregate booleans.
    pub fn new(approved: bool, changes_requested: bool) -> Self {
        Self {
            approved,
            changes_requested,
        }
    }

    /// Builds a status from the Forge aggregate review model.
    pub fn from_aggregate(status: &PullRequestReviewStatus) -> Self {
        Self::new(status.is_approved(), status.has_changes_requested())
    }

    /// Returns whether the native review aggregate is approved.
    pub fn is_approved(&self) -> bool {
        self.approved
    }

    /// Returns whether a latest native review decision requests changes.
    pub fn has_changes_requested(&self) -> bool {
        self.changes_requested
    }
}

/// The runtime signals a transition plan may consult.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GateSignals {
    dependencies: DependencyStatus,
    ci: CiStatus,
    review: ReviewStatus,
}

impl GateSignals {
    /// An empty bundle: nothing landed and CI pending/unknown.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the dependency status.
    pub fn with_dependencies(mut self, dependencies: DependencyStatus) -> Self {
        self.dependencies = dependencies;
        self
    }

    /// Sets the CI status.
    pub fn with_ci(mut self, ci: CiStatus) -> Self {
        self.ci = ci;
        self
    }

    /// Sets the review status.
    pub fn with_review(mut self, review: ReviewStatus) -> Self {
        self.review = review;
        self
    }

    /// Returns the dependency status.
    pub fn dependencies(&self) -> &DependencyStatus {
        &self.dependencies
    }

    /// Returns the CI status.
    pub fn ci(&self) -> &CiStatus {
        &self.ci
    }

    /// Returns the review status.
    pub fn review(&self) -> &ReviewStatus {
        &self.review
    }
}
