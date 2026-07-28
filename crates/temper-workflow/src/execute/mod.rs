//! Runtime execution of planned transitions through a [`Forge`] (Phase 6).
//!
//! The [`plan`](crate::plan) module decides *whether* a transition is allowed
//! and *what* effects it would produce, but it never touches a backend. This
//! module is the first runtime layer that applies those effects against a real
//! [`Forge`], following the runtime guarantees in
//! `docs/reference/workflow-layer.md`:
//!
//! 1. load fresh Forge state for the target artifact,
//! 2. classify it under the validated workflow,
//! 3. re-check role authority and transition preconditions (via the planner),
//! 4. apply the planned effects through the [`Forge`] trait,
//! 5. verify the transition's postconditions against the committed update result.
//!
//! # Non-label effects
//!
//! Besides label add/remove, the executor applies `SetAssignee`,
//! `RemoveAssignee`, `CreateComment`, `CreatePullRequest`, and
//! `MergePullRequest`. Assignee effects name a workflow *role*; the
//! [`ExecutionContext`] supplies the role→Forge-user binding, and an unbound
//! role fails with [`ExecutionError::UnresolvedAssignee`] before any mutation.
//! Assignee changes are folded into the same single label update so the state
//! flip is one atomic backend call.
//!
//! Comments are not naturally idempotent, so each planned comment is stamped
//! with a deterministic marker derived from `(transition, comment-index)` and
//! posted only when no existing comment already carries that marker. Comments
//! are posted *before* the label/assignee update so that the label flip remains
//! the commit point: a crash before it leaves preconditions intact (a retry
//! re-plans, the marker dedupes the comment, the flip commits), and a crash
//! after it leaves the transition fully applied (a retry sees stale
//! preconditions and is correctly refused).
//!
//! `CreatePullRequest` is applied through [`Executor::ensure_pull_request`]. The
//! effect may supply the correlation key and an artifact kind for stable lookup;
//! [`ExecutionContext`] supplies the concrete title, body, branches, labels, and
//! assignees. The create runs before
//! the label/assignee commit point so a retry after a landed create reuses the
//! existing pull request rather than duplicating it.
//!
//! # Merge and post-merge projection
//!
//! `MergePullRequest` is applied through the [`Forge`] merge API. It runs
//! *before* the label/assignee commit point and is guarded by the freshly
//! loaded pull-request state: a pull request that is already merged is skipped,
//! so the merge is at most once even when a crash lands the merge but loses the
//! response. If the backend reports a merge conflict/rejection, the executor
//! re-reads the pull request before deciding: already merged continues to
//! post-merge projection, missing or closed is stale, and still-open/unmerged is
//! returned as [`ExecutionError::MergeConflict`] for declared workflow routing.
//! Direct mechanical automation merges additionally request source-branch
//! cleanup from backends that support merge-time branch deletion.
//! The transition's post-merge labels (`landed`, `alignment`) are modeled as
//! ordinary `add_label` effects, so they are projected by the same atomic update
//! and survive on the now-closed pull request — there is no executor-special-
//! cased post-merge labeling. Lease effects remain unsupported until later
//! phases.
//!
//! # Gate signals
//!
//! Before planning, the executor reads gate facts from fresh Forge state into
//! [`GateSignals`](crate::plan::GateSignals). Dependency gates are fed by native
//! dependency targets (closed issues or merged pull requests), CI gate/queue
//! conditions are fed by native CI jobs from
//! [`Forge::list_ci_jobs`](temper_forge::Forge::list_ci_jobs) (see
//! [`CiStatus::from_jobs`](crate::plan::CiStatus::from_jobs), ADR 0014, and ADR
//! 0017), and review gates are fed by requested reviewers plus native review
//! events (ADR 0016).
//!
//! Reloading and re-planning before every mutation is deliberate: Forge state
//! can be edited by humans or other workers between planning and execution, so
//! the executor never trusts a plan computed against stale state. It always
//! re-plans against the freshly loaded artifact.
//!
//! The executor is generic over `F: Forge + ?Sized`, so it works with a
//! concrete backend such as `temper_forge_filesystem::FilesystemForge`,
//! `temper_forge_memory::MemoryForge`, or a `&dyn Forge` trait object.
//!
//! # Idempotent create
//!
//! The current [`Forge`] interface has no native create-once primitive, so
//! [`Executor::ensure_issue`], [`Executor::ensure_issue_with_parent`], and
//! [`Executor::ensure_pull_request`] implement idempotency in the workflow
//! layer: they stamp a
//! [correlation key](crate::metadata::WorkflowMetadata::correlation_key) into
//! the new artifact's metadata block, then search with bounded summary list
//! queries over explicit states, create labels, and a body marker before
//! creating. The body marker is only a narrowing hint; the executor parses the
//! metadata block and compares the exact key before accepting a match. Retrying
//! with the same key returns the existing artifact instead of creating a
//! duplicate.

mod apply;
mod audit;
mod ensure;
mod error;
mod issue_creates;
mod merge;
mod messaging;
mod signals;
mod types;
mod verify;

pub use ensure::{
    CorrelationLookupPlan, find_pull_request_by_correlation, validate_pull_request_topology,
};
pub use error::ExecutionError;
pub use types::{EnsureOutcome, ExecutionReport};

use crate::classify::{ArtifactSource, ClassifiedArtifact, Classifier};
use crate::context::ExecutionContext;
use crate::ids::{RoleId, TransitionId};
use crate::plan::TransitionPlan;
use crate::validated::{Effect, ValidatedWorkflow};
use async_trait::async_trait;
use error::classify_plan_error;
use std::sync::Arc;
use temper_forge::{
    Forge, Issue, IssueId, ItemListDetails, PullRequestId, PullRequestState, RepositoryId, UserId,
    Version,
};

/// A loaded Forge artifact with the handle needed to mutate it.
enum Loaded {
    Issue {
        id: IssueId,
        version: Version,
        snapshot: Issue,
        classified: ClassifiedArtifact,
    },
    PullRequest {
        id: PullRequestId,
        /// Whether the freshly loaded pull request is already merged. Lets the
        /// merge effect be at-most-once: an already-merged target is skipped.
        merged: bool,
        /// Whether the pull request is in a terminal Forge state (merged or
        /// closed). CI status is meaningless for a terminal PR — no CI-gated
        /// transition can fire — so the (expensive) CI read is skipped for it
        /// (see [`Executor::gate_signals`]). A merged/closed PR still carrying a
        /// workflow label is pulled into bounded reconciliation, so without this
        /// guard every historical PR's CI is re-read on every mechanical tick.
        terminal: bool,
        /// Head commit SHA, when the backend records one. Scopes the CI signal
        /// to the pull request's head commit (see [`Executor::gate_signals`]).
        head_sha: Option<String>,
        /// Fresh source branch from the provider's exact PR representation.
        source_branch: String,
        /// Users whose native review has been requested.
        requested_reviewers: Vec<UserId>,
        classified: ClassifiedArtifact,
    },
}

impl Loaded {
    fn classified(&self) -> &ClassifiedArtifact {
        match self {
            Loaded::Issue { classified, .. } | Loaded::PullRequest { classified, .. } => classified,
        }
    }

    fn issue_snapshot(&self) -> Option<&Issue> {
        match self {
            Loaded::Issue { snapshot, .. } => Some(snapshot),
            Loaded::PullRequest { .. } => None,
        }
    }
}

/// Durable checkpoints in the staged child-issue lifecycle.
///
/// Hooks run after a Forge mutation has committed and before the next pass can
/// durably subsume it. This makes them useful for crash injection: replay must
/// discover the committed mutation and continue without creating, relating, or
/// dispatching a duplicate child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildIssueCheckpoint {
    Created,
    Wired,
    ParentAggregated,
    Activated,
    Completed,
}

/// Optional observer for child issue lifecycle checkpoints.
///
/// Production executors install no observer. Integration stacks may install a
/// channel-backed observer to stop exactly between a committed child mutation
/// and its durable intent progress update.
#[async_trait]
pub trait ChildIssueLifecycleHook: Send + Sync {
    async fn reached(&self, checkpoint: ChildIssueCheckpoint);
}

/// Applies planned workflow transitions against a [`Forge`] backend.
///
/// Bound to a [`ValidatedWorkflow`] (never a raw spec) and a backend handle.
/// Every mutation re-loads, re-classifies, and re-plans against fresh state, so
/// a single [`Executor`] is safe to reuse across many executions.
pub struct Executor<'a, F: Forge + ?Sized> {
    workflow: &'a ValidatedWorkflow,
    forge: &'a F,
    context: ExecutionContext,
    child_issue_hook: Option<Arc<dyn ChildIssueLifecycleHook>>,
}

impl<'a, F: Forge + ?Sized> Executor<'a, F> {
    /// Creates an executor bound to a validated workflow and a backend with an
    /// empty [`ExecutionContext`].
    ///
    /// Use [`Executor::with_context`] when a transition can plan assignee
    /// effects, so role→user resolution is available.
    pub fn new(workflow: &'a ValidatedWorkflow, forge: &'a F) -> Self {
        Self::with_context(workflow, forge, ExecutionContext::new())
    }

    /// Creates an executor with an explicit [`ExecutionContext`] supplying the
    /// role→Forge-user bindings that assignee effects need.
    pub fn with_context(
        workflow: &'a ValidatedWorkflow,
        forge: &'a F,
        context: ExecutionContext,
    ) -> Self {
        Self {
            workflow,
            forge,
            context,
            child_issue_hook: None,
        }
    }

    /// Installs an observer for committed child lifecycle checkpoints.
    #[must_use]
    pub fn with_child_issue_hook(mut self, hook: Arc<dyn ChildIssueLifecycleHook>) -> Self {
        self.child_issue_hook = Some(hook);
        self
    }

    async fn child_issue_checkpoint(&self, checkpoint: ChildIssueCheckpoint) {
        if let Some(hook) = &self.child_issue_hook {
            hook.reached(checkpoint).await;
        }
    }

    /// Executes a transition for a role against a target Forge artifact.
    ///
    /// Loads fresh state, classifies it, re-plans the transition (re-checking
    /// authority, preconditions, gates, and resulting states), applies the
    /// planned effects, and verifies the postconditions against the committed
    /// update result. Returns an [`ExecutionReport`] on success or a typed
    /// [`ExecutionError`] identifying the failed stage. No mutation occurs
    /// unless planning succeeds.
    pub async fn execute(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        transition: &TransitionId,
        role: &RoleId,
    ) -> Result<ExecutionReport, ExecutionError> {
        let needs = self.workflow.signal_needs_for_transition(transition);
        let issue_details = if needs.dependencies {
            ItemListDetails::full()
        } else {
            ItemListDetails::summary()
        };
        let loaded = self
            .load_with_issue_details(repo_id, target, issue_details)
            .await?;
        // A recovery marker fences the whole landing transition, not just the
        // merge call. Executor pre-effects include comments, PR creates,
        // reviews, and parent updates, so waiting until `apply_merge` would
        // permit partial publication before refusing the landing.
        let is_landing = self.workflow.transitions().iter().any(|candidate| {
            &candidate.id == transition
                && candidate
                    .effects
                    .iter()
                    .any(|effect| matches!(effect, Effect::MergePullRequest))
        });
        if is_landing && loaded.classified().metadata.provider_recovery.is_some() {
            return Err(ExecutionError::TargetStale {
                target,
                message: "provider recovery is deferred; mechanical landing is fenced".to_string(),
            });
        }
        let signals = self
            .gate_signals_with_needs(repo_id, &loaded, needs)
            .await?;

        let plan = self
            .workflow
            .planner()
            .plan_transition_with(transition, role, loaded.classified(), &signals)
            .map_err(classify_plan_error)?;

        // `apply` validates every effect (rejecting unsupported effects,
        // missing create inputs, and unbound assignee roles) before it mutates
        // anything, posts idempotent comments, ensures pull requests, then
        // folds labels and assignees into a single update. A transition
        // therefore never partially applies its label/assignee flip, and its
        // committed update result is used for postcondition checks so a later
        // worker cannot make this transition look unsuccessful.
        self.apply(repo_id, &loaded, &plan).await?;

        Ok(ExecutionReport {
            transition: plan.transition,
            role: plan.role,
            target,
            applied: plan.effects,
        })
    }

    /// Plans a transition against fresh state without applying anything.
    ///
    /// Loads and classifies the target, then re-checks authority, preconditions,
    /// gates, and resulting states through the [`planner`](crate::plan::Planner).
    /// Returns the [`TransitionPlan`] that [`execute`](Self::execute) would
    /// apply, so callers can preview effects or journal a command's intent
    /// before mutating. Like `execute`, it never trusts stale state: a plan is
    /// only valid against the state it was loaded from.
    pub async fn plan(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        transition: &TransitionId,
        role: &RoleId,
    ) -> Result<TransitionPlan, ExecutionError> {
        let needs = self.workflow.signal_needs_for_transition(transition);
        let issue_details = if needs.dependencies {
            ItemListDetails::full()
        } else {
            ItemListDetails::summary()
        };
        let loaded = self
            .load_with_issue_details(repo_id, target, issue_details)
            .await?;
        let signals = self
            .gate_signals_with_needs(repo_id, &loaded, needs)
            .await?;
        self.workflow
            .planner()
            .plan_transition_with(transition, role, loaded.classified(), &signals)
            .map_err(classify_plan_error)
    }

    /// Loads and classifies the target artifact from fresh Forge state.
    async fn load(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
    ) -> Result<Loaded, ExecutionError> {
        self.load_with_issue_details(repo_id, target, ItemListDetails::full())
            .await
    }

    async fn load_with_issue_details(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        issue_details: ItemListDetails,
    ) -> Result<Loaded, ExecutionError> {
        let classifier = Classifier::new(self.workflow);
        match target {
            ArtifactSource::Issue { number } => {
                let issue = self
                    .forge
                    .get_issue_by_number_with_details(repo_id, number, issue_details)
                    .await?
                    .ok_or(ExecutionError::TargetMissing { target })?;
                let classified = classifier
                    .classify_issue(&issue)
                    .map_err(ExecutionError::Classification)?;
                Ok(Loaded::Issue {
                    id: issue.id.clone(),
                    version: issue.version,
                    snapshot: issue,
                    classified,
                })
            }
            ArtifactSource::PullRequest { number } => {
                let pull_request = self
                    .forge
                    .get_pull_request_by_number(repo_id, number)
                    .await?
                    .ok_or(ExecutionError::TargetMissing { target })?;
                let merged = matches!(pull_request.state, PullRequestState::Merged);
                let terminal = matches!(
                    pull_request.state,
                    PullRequestState::Merged | PullRequestState::Closed
                );
                let head_sha = pull_request.head_sha.clone();
                let source_branch = pull_request.source.branch.clone();
                let requested_reviewers = pull_request.requested_reviewers.clone();
                let classified = classifier
                    .classify_pull_request(&pull_request)
                    .map_err(ExecutionError::Classification)?;
                Ok(Loaded::PullRequest {
                    id: pull_request.id,
                    merged,
                    terminal,
                    head_sha,
                    source_branch,
                    requested_reviewers,
                    classified,
                })
            }
        }
    }
}

impl ValidatedWorkflow {
    /// Returns an [`Executor`] bound to this workflow and a backend.
    ///
    /// Convenience wrapper around [`Executor::new`]; the executor has an empty
    /// [`ExecutionContext`].
    pub fn executor<'a, F: Forge + ?Sized>(&'a self, forge: &'a F) -> Executor<'a, F> {
        Executor::new(self, forge)
    }

    /// Returns an [`Executor`] bound to this workflow, a backend, and an
    /// explicit [`ExecutionContext`] for assignee resolution.
    pub fn executor_with_context<'a, F: Forge + ?Sized>(
        &'a self,
        forge: &'a F,
        context: ExecutionContext,
    ) -> Executor<'a, F> {
        Executor::with_context(self, forge, context)
    }
}
