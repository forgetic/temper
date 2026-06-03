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
//! effect supplies the correlation key; [`ExecutionContext`] supplies the
//! concrete title, body, branches, labels, and assignees. The create runs before
//! the label/assignee commit point so a retry after a landed create reuses the
//! existing pull request rather than duplicating it.
//!
//! # Merge and post-merge projection
//!
//! `MergePullRequest` is applied through the [`Forge`] merge API. It runs
//! *before* the label/assignee commit point and is guarded by the freshly
//! loaded pull-request state: a pull request that is already merged is skipped,
//! so the merge is at most once even when a crash lands the merge but loses the
//! response. The transition's post-merge labels (`landed`, `alignment`) are
//! modeled as ordinary `add_label` effects, so they are projected by the same
//! atomic update and survive on the now-closed pull request — there is no
//! executor-special-cased post-merge labeling. Lease effects remain unsupported
//! until later phases.
//!
//! # Gate signals
//!
//! Before planning, the executor reads gate facts from fresh Forge state into
//! [`GateSignals`]. Dependency gates are fed by native dependency targets
//! (closed issues or merged pull requests), CI gate/queue conditions are fed by
//! native CI jobs from [`Forge::list_ci_jobs`](temper_forge::Forge::list_ci_jobs)
//! (see [`CiStatus::from_jobs`], ADR 0014, and ADR 0017), and review gates are
//! fed by requested reviewers plus native review events (ADR 0016).
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
//! the new artifact's metadata block and search existing artifacts for that key
//! before creating. Retrying with the same key returns the existing artifact
//! instead of creating a duplicate.

mod apply;
mod ensure;
mod signals;

use crate::classify::{ArtifactSource, ClassificationError, ClassifiedArtifact, Classifier};
use crate::context::ExecutionContext;
use crate::ids::{RoleId, TransitionId};
use crate::plan::{PlanDiagnostic, PlanError, Postcondition, TransitionPlan, WorkflowEffect};
use crate::validated::ValidatedWorkflow;
use temper_forge::{
    Forge, ForgeError, IssueId, PullRequestId, PullRequestState, RepositoryId, UserId,
};

/// Outcome of an idempotent ensure-create operation.
///
/// Distinguishes whether the executor found an existing artifact with the
/// requested correlation key or created a fresh one, so callers and tests can
/// assert that a retry did not duplicate the artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnsureOutcome<T> {
    /// A matching artifact already existed; no new artifact was created.
    Existing(T),
    /// No matching artifact existed, so a new one was created.
    Created(T),
}

impl<T> EnsureOutcome<T> {
    /// Borrows the resolved artifact, whether found or created.
    pub fn artifact(&self) -> &T {
        match self {
            EnsureOutcome::Existing(artifact) | EnsureOutcome::Created(artifact) => artifact,
        }
    }

    /// Returns `true` when a new artifact was created.
    pub fn was_created(&self) -> bool {
        matches!(self, EnsureOutcome::Created(_))
    }

    /// Consumes the outcome and returns the resolved artifact.
    pub fn into_artifact(self) -> T {
        match self {
            EnsureOutcome::Existing(artifact) | EnsureOutcome::Created(artifact) => artifact,
        }
    }
}

/// A successful transition execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionReport {
    /// Transition that was executed.
    pub transition: TransitionId,
    /// Role the execution was authorized for.
    pub role: RoleId,
    /// Forge artifact the effects were applied to.
    pub target: ArtifactSource,
    /// Effects applied to the artifact, in plan order.
    pub applied: Vec<WorkflowEffect>,
}

/// Why a transition execution failed.
///
/// The variants deliberately separate the three failure classes the runtime
/// must distinguish: a [validation](ExecutionError::Validation) problem with the
/// request itself, a [precondition](ExecutionError::Precondition) problem with
/// the artifact's current state, and a [backend](ExecutionError::Backend)
/// failure from the Forge. Classification, missing-target, unsupported-effect,
/// missing-create-context, and postcondition failures are reported distinctly so
/// callers never have to guess which stage failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionError {
    /// The request is invalid regardless of artifact state: an undeclared
    /// transition, an unauthorized role, or an artifact-kind mismatch.
    Validation { diagnostics: Vec<PlanDiagnostic> },
    /// The artifact's fresh state forbids the transition: a stale or
    /// contradicted label precondition, an unsatisfied gate, or an impossible
    /// resulting state. No mutation is performed.
    Precondition { diagnostics: Vec<PlanDiagnostic> },
    /// Fresh Forge state could not be classified under the workflow.
    Classification(ClassificationError),
    /// The target artifact does not exist in the backend.
    TargetMissing { target: ArtifactSource },
    /// The planner produced an effect the executor cannot apply yet.
    UnsupportedEffect { effect: WorkflowEffect },
    /// An assignee effect named a role with no Forge user bound in the
    /// [`ExecutionContext`]. Reported before any mutation.
    UnresolvedAssignee { role: RoleId },
    /// A reviewer-request effect named a role with no Forge user bound.
    UnresolvedReviewer { role: RoleId },
    /// A `CreatePullRequest` effect omitted the correlation key needed for
    /// idempotent execution. Reported before any mutation.
    MissingCorrelationKey { effect: WorkflowEffect },
    /// A `CreatePullRequest` effect has no concrete create input bound in the
    /// [`ExecutionContext`]. Reported before any mutation.
    UnresolvedPullRequestCreate {
        transition: TransitionId,
        effect_index: usize,
    },
    /// A postcondition did not hold after the effects were applied.
    PostconditionFailed { postcondition: Postcondition },
    /// A backend operation failed.
    Backend { message: String },
}

impl std::fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionError::Validation { diagnostics } => {
                write!(formatter, "transition request is invalid:")?;
                write_diagnostics(formatter, diagnostics)
            }
            ExecutionError::Precondition { diagnostics } => {
                write!(formatter, "transition preconditions are not met:")?;
                write_diagnostics(formatter, diagnostics)
            }
            ExecutionError::Classification(error) => {
                write!(formatter, "could not classify fresh state: {error}")
            }
            ExecutionError::TargetMissing { target } => {
                write!(formatter, "target artifact {target:?} does not exist")
            }
            ExecutionError::UnsupportedEffect { effect } => {
                write!(formatter, "executor cannot apply effect {effect:?}")
            }
            ExecutionError::UnresolvedAssignee { role } => {
                write!(
                    formatter,
                    "no Forge user is bound for assignee role `{role}`"
                )
            }
            ExecutionError::UnresolvedReviewer { role } => {
                write!(formatter, "no Forge user is bound for reviewer role `{role}`")
            }
            ExecutionError::MissingCorrelationKey { effect } => {
                write!(formatter, "effect {effect:?} has no correlation key")
            }
            ExecutionError::UnresolvedPullRequestCreate {
                transition,
                effect_index,
            } => write!(
                formatter,
                "no pull-request create input is bound for transition `{transition}` create effect #{effect_index}"
            ),
            ExecutionError::PostconditionFailed { postcondition } => {
                write!(
                    formatter,
                    "postcondition not satisfied after applying effects: {postcondition:?}"
                )
            }
            ExecutionError::Backend { message } => {
                write!(formatter, "backend error: {message}")
            }
        }
    }
}

impl std::error::Error for ExecutionError {}

fn write_diagnostics(
    formatter: &mut std::fmt::Formatter<'_>,
    diagnostics: &[PlanDiagnostic],
) -> std::fmt::Result {
    for diagnostic in diagnostics {
        write!(formatter, "\n  - {diagnostic}")?;
    }
    Ok(())
}

impl From<ForgeError> for ExecutionError {
    fn from(error: ForgeError) -> Self {
        ExecutionError::Backend {
            message: error.to_string(),
        }
    }
}

/// Splits a [`PlanError`] into the matching [`ExecutionError`] class.
///
/// A request-level problem (unknown transition, unauthorized role, kind
/// mismatch) outranks a state-level problem, so a mixed error is reported as a
/// validation failure. Otherwise every diagnostic is state-level and the error
/// is a precondition failure.
fn classify_plan_error(error: PlanError) -> ExecutionError {
    let diagnostics = error.diagnostics().to_vec();
    if diagnostics.iter().any(is_validation_diagnostic) {
        ExecutionError::Validation { diagnostics }
    } else {
        ExecutionError::Precondition { diagnostics }
    }
}

fn is_validation_diagnostic(diagnostic: &PlanDiagnostic) -> bool {
    matches!(
        diagnostic,
        PlanDiagnostic::UnknownTransition { .. }
            | PlanDiagnostic::Unauthorized { .. }
            | PlanDiagnostic::ArtifactKindMismatch { .. }
    )
}

/// A loaded Forge artifact with the handle needed to mutate it.
enum Loaded {
    Issue {
        id: IssueId,
        classified: ClassifiedArtifact,
    },
    PullRequest {
        id: PullRequestId,
        /// Whether the freshly loaded pull request is already merged. Lets the
        /// merge effect be at-most-once: an already-merged target is skipped.
        merged: bool,
        /// Head commit SHA, when the backend records one. Scopes the CI signal
        /// to the pull request's head commit (see [`Executor::gate_signals`]).
        head_sha: Option<String>,
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
        let loaded = self.load(repo_id, target).await?;
        let needs = self.workflow.signal_needs_for_transition(transition);
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
        let loaded = self.load(repo_id, target).await?;
        let needs = self.workflow.signal_needs_for_transition(transition);
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
        let classifier = Classifier::new(self.workflow);
        match target {
            ArtifactSource::Issue { number } => {
                let issue = self
                    .forge
                    .get_issue_by_number(repo_id, number)
                    .await?
                    .ok_or(ExecutionError::TargetMissing { target })?;
                let classified = classifier
                    .classify_issue(&issue)
                    .map_err(ExecutionError::Classification)?;
                Ok(Loaded::Issue {
                    id: issue.id,
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
                let head_sha = pull_request.head_sha.clone();
                let requested_reviewers = pull_request.requested_reviewers.clone();
                let classified = classifier
                    .classify_pull_request(&pull_request)
                    .map_err(ExecutionError::Classification)?;
                Ok(Loaded::PullRequest {
                    id: pull_request.id,
                    merged,
                    head_sha,
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
