//! Runtime-supplied gate signals for the planner.
//!
//! Some gate conditions ask about facts the pure planner must not derive
//! itself: whether prerequisite work has landed (`dependencies_resolved`),
//! whether native CI passed (`ci_passed`), and whether native reviews approve or
//! request changes. Those facts are read fresh from the Forge by the runtime and
//! supplied to the planner as a small signal bundle.
//!
//! [`GateSignals`] bundles every runtime signal a transition plan may consult.
//! Bundling (rather than threading one parameter per gate) keeps adding signals
//! from re-threading every planner call site, and gives one obvious place to
//! construct "the facts the runtime read before planning". The [`Planner`] only
//! reads the bundle; it never lists jobs, reviews, or talks to a Forge.
//!
//! [`Planner`]: super::Planner

use super::DependencyStatus;
use harness_forge::{CiJob, CiJobConclusion, CiJobStatus, PullRequestReviewStatus};
use std::collections::HashMap;

/// Runtime-supplied verdict on whether an artifact's native CI passed.
///
/// `ci_passed` asks whether the artifact's CI is green. Like the dependency
/// signal, the verdict is decided by the runtime — never derived inside the
/// pure planner — and supplied here as a thin boolean the planner only reads.
/// Build it from fresh [`CiJob`]s with [`CiStatus::from_jobs`], which is the one
/// documented place the pass rule lives.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CiStatus {
    passed: bool,
}

impl CiStatus {
    /// A status whose CI has not passed (the default, conservative verdict).
    pub fn new() -> Self {
        Self::default()
    }

    /// A status whose CI has passed.
    pub fn passed() -> Self {
        Self { passed: true }
    }

    /// Builds a status directly from a pass/fail verdict.
    pub fn with_passed(passed: bool) -> Self {
        Self { passed }
    }

    /// Returns whether CI has passed.
    pub fn is_passed(&self) -> bool {
        self.passed
    }

    /// Computes the CI verdict from a set of CI jobs (the documented pass rule).
    ///
    /// The jobs are reduced to the latest job per name (by `created_at`). CI is
    /// *passed* when that set is non-empty and every latest-per-name job has
    /// status [`CiJobStatus::Completed`] with conclusion
    /// [`CiJobConclusion::Success`]. Any latest job that is still
    /// `Queued`/`Running`, or concluded anything other than `Success`, leaves CI
    /// *not passed*. A pull request with no CI jobs is therefore *not passed*:
    /// the merge gate does not open before CI has run. See ADR 0014.
    pub fn from_jobs(jobs: &[CiJob]) -> Self {
        let mut latest: HashMap<&str, &CiJob> = HashMap::new();
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
        let passed = !latest.is_empty()
            && latest.values().all(|job| {
                job.status == CiJobStatus::Completed
                    && job.conclusion == Some(CiJobConclusion::Success)
            });
        Self { passed }
    }
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
    /// An empty bundle: nothing landed and CI not passed.
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
