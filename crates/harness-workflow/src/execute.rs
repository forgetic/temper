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
//! 5. verify the transition's postconditions against fresh state.
//!
//! Reloading and re-planning before every mutation is deliberate: Forge state
//! can be edited by humans or other workers between planning and execution, so
//! the executor never trusts a plan computed against stale state. It always
//! re-plans against the freshly loaded artifact.
//!
//! The executor is generic over `F: Forge + ?Sized`, so it works with a
//! concrete backend such as `harness_fs::FilesystemForge` or with a
//! `&dyn Forge` trait object.
//!
//! # Idempotent create
//!
//! The current [`Forge`] interface has no native create-once primitive, so
//! [`Executor::ensure_issue`] implements idempotency in the workflow layer: it
//! stamps a [correlation key](crate::metadata::WorkflowMetadata::correlation_key)
//! into the new artifact's metadata block and searches existing artifacts for
//! that key before creating. Retrying with the same key returns the existing
//! artifact instead of creating a duplicate.

use crate::classify::{ArtifactSource, ClassificationError, ClassifiedArtifact, Classifier};
use crate::ids::{RoleId, TransitionId};
use crate::metadata::{
    parse_metadata_block, render_metadata_block, WorkflowMetadata, METADATA_BEGIN, METADATA_END,
};
use crate::plan::{PlanDiagnostic, PlanError, Postcondition, WorkflowEffect};
use crate::validated::ValidatedWorkflow;
use harness_forge::{
    CreateIssue, Forge, ForgeError, Issue, IssueId, IssueQuery, PullRequestId, RepositoryId,
    UpdateIssue, UpdatePullRequest,
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
/// and postcondition failures are reported distinctly so callers never have to
/// guess which stage failed.
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
}

impl<'a, F: Forge + ?Sized> Executor<'a, F> {
    /// Creates an executor bound to a validated workflow and a backend.
    pub fn new(workflow: &'a ValidatedWorkflow, forge: &'a F) -> Self {
        Self { workflow, forge }
    }

    /// Executes a transition for a role against a target Forge artifact.
    ///
    /// Loads fresh state, classifies it, re-plans the transition (re-checking
    /// authority, preconditions, gates, and resulting states), applies the
    /// planned effects, and verifies the postconditions. Returns an
    /// [`ExecutionReport`] on success or a typed [`ExecutionError`] identifying
    /// the failed stage. No mutation occurs unless planning succeeds.
    pub async fn execute(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        transition: &TransitionId,
        role: &RoleId,
    ) -> Result<ExecutionReport, ExecutionError> {
        let loaded = self.load(repo_id, target).await?;

        let plan = self
            .workflow
            .planner()
            .plan_transition(transition, role, loaded.classified())
            .map_err(classify_plan_error)?;

        // `apply` folds the effects into a single update and rejects any
        // unsupported effect before it issues that update, so a transition can
        // never partially apply.
        self.apply(&loaded, &plan.effects).await?;
        self.verify(repo_id, target, &plan.postconditions).await?;

        Ok(ExecutionReport {
            transition: plan.transition,
            role: plan.role,
            target,
            applied: plan.effects,
        })
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
                let classified = classifier
                    .classify_pull_request(&pull_request)
                    .map_err(ExecutionError::Classification)?;
                Ok(Loaded::PullRequest {
                    id: pull_request.id,
                    classified,
                })
            }
        }
    }

    /// Applies the planned label effects with a single backend update.
    ///
    /// Effects are folded into one `add_labels`/`remove_labels` update so the
    /// mutation is a single backend call per artifact rather than one call per
    /// label.
    async fn apply(
        &self,
        loaded: &Loaded,
        effects: &[WorkflowEffect],
    ) -> Result<(), ExecutionError> {
        let mut add_labels = Vec::new();
        let mut remove_labels = Vec::new();
        for effect in effects {
            match label_change(effect) {
                Some((LabelChange::Add, label)) => add_labels.push(label),
                Some((LabelChange::Remove, label)) => remove_labels.push(label),
                None => {
                    return Err(ExecutionError::UnsupportedEffect {
                        effect: effect.clone(),
                    })
                }
            }
        }

        match loaded {
            Loaded::Issue { id, .. } => {
                let update = UpdateIssue {
                    add_labels,
                    remove_labels,
                    ..UpdateIssue::default()
                };
                self.forge.update_issue(id, update).await?;
            }
            Loaded::PullRequest { id, .. } => {
                let update = UpdatePullRequest {
                    add_labels,
                    remove_labels,
                    ..UpdatePullRequest::default()
                };
                self.forge.update_pull_request(id, update).await?;
            }
        }
        Ok(())
    }

    /// Reloads fresh state and checks every postcondition holds.
    async fn verify(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        postconditions: &[Postcondition],
    ) -> Result<(), ExecutionError> {
        let labels = self.current_labels(repo_id, target).await?;
        for postcondition in postconditions {
            let satisfied = match postcondition {
                Postcondition::LabelPresent(label) => labels.iter().any(|l| l == label.as_str()),
                Postcondition::LabelAbsent(label) => labels.iter().all(|l| l != label.as_str()),
            };
            if !satisfied {
                return Err(ExecutionError::PostconditionFailed {
                    postcondition: postcondition.clone(),
                });
            }
        }
        Ok(())
    }

    /// Reads the artifact's current labels from fresh Forge state.
    async fn current_labels(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
    ) -> Result<Vec<String>, ExecutionError> {
        match target {
            ArtifactSource::Issue { number } => Ok(self
                .forge
                .get_issue_by_number(repo_id, number)
                .await?
                .ok_or(ExecutionError::TargetMissing { target })?
                .labels),
            ArtifactSource::PullRequest { number } => Ok(self
                .forge
                .get_pull_request_by_number(repo_id, number)
                .await?
                .ok_or(ExecutionError::TargetMissing { target })?
                .labels),
        }
    }

    /// Idempotently ensures an issue exists for a correlation key.
    ///
    /// Searches existing issues for one whose metadata block carries
    /// `correlation_key`; if found, returns it unchanged. Otherwise stamps the
    /// key into the new issue's metadata block and creates it. Retrying with the
    /// same key therefore returns the existing issue instead of duplicating it.
    pub async fn ensure_issue(
        &self,
        repo_id: &RepositoryId,
        correlation_key: &str,
        input: CreateIssue,
    ) -> Result<EnsureOutcome<Issue>, ExecutionError> {
        if let Some(existing) = self
            .find_issue_by_correlation(repo_id, correlation_key)
            .await?
        {
            return Ok(EnsureOutcome::Existing(existing));
        }

        let body = body_with_correlation_key(&input.body, correlation_key)
            .map_err(|message| ExecutionError::Backend { message })?;
        let created = self
            .forge
            .create_issue(repo_id, CreateIssue { body, ..input })
            .await?;
        Ok(EnsureOutcome::Created(created))
    }

    /// Finds an issue whose metadata block carries the correlation key.
    async fn find_issue_by_correlation(
        &self,
        repo_id: &RepositoryId,
        correlation_key: &str,
    ) -> Result<Option<Issue>, ExecutionError> {
        let issues = self
            .forge
            .list_issues(repo_id, IssueQuery::default())
            .await?;
        Ok(issues.into_iter().find(|issue| {
            matches!(
                parse_metadata_block(&issue.body),
                Ok(Some(WorkflowMetadata {
                    correlation_key: Some(ref key),
                    ..
                })) if key == correlation_key
            )
        }))
    }
}

/// Whether a label effect adds or removes its label.
enum LabelChange {
    Add,
    Remove,
}

/// Extracts the label mutation from an effect, or `None` for non-label effects.
///
/// Only [`WorkflowEffect::AddLabel`] and [`WorkflowEffect::RemoveLabel`] are
/// applied by this executor today; every other variant is a planner placeholder
/// (see [`crate::plan::WorkflowEffect`]) and maps to `None`, which the executor
/// reports as [`ExecutionError::UnsupportedEffect`] before mutating anything.
fn label_change(effect: &WorkflowEffect) -> Option<(LabelChange, String)> {
    match effect {
        WorkflowEffect::AddLabel(label) => Some((LabelChange::Add, label.as_str().to_string())),
        WorkflowEffect::RemoveLabel(label) => {
            Some((LabelChange::Remove, label.as_str().to_string()))
        }
        _ => None,
    }
}

/// Returns `body` with `correlation_key` set in its metadata block.
///
/// If the body already has a metadata block, the key is set in place; otherwise
/// a fresh block is appended. The result round-trips through
/// [`parse_metadata_block`], so a later search can find the artifact.
fn body_with_correlation_key(body: &str, correlation_key: &str) -> Result<String, String> {
    match parse_metadata_block(body).map_err(|error| error.to_string())? {
        Some(mut metadata) => {
            metadata.correlation_key = Some(correlation_key.to_string());
            let start = body
                .find(METADATA_BEGIN)
                .expect("metadata block was just parsed");
            let after = &body[start + METADATA_BEGIN.len()..];
            let end = after
                .find(METADATA_END)
                .expect("metadata block is terminated");
            let block_end = start + METADATA_BEGIN.len() + end + METADATA_END.len();
            Ok(format!(
                "{}{}{}",
                &body[..start],
                render_metadata_block(&metadata),
                &body[block_end..]
            ))
        }
        None => {
            let metadata = WorkflowMetadata {
                correlation_key: Some(correlation_key.to_string()),
                ..WorkflowMetadata::default()
            };
            let block = render_metadata_block(&metadata);
            if body.is_empty() {
                Ok(block)
            } else {
                Ok(format!("{body}\n\n{block}"))
            }
        }
    }
}

impl ValidatedWorkflow {
    /// Returns an [`Executor`] bound to this workflow and a backend.
    ///
    /// Convenience wrapper around [`Executor::new`].
    pub fn executor<'a, F: Forge + ?Sized>(&'a self, forge: &'a F) -> Executor<'a, F> {
        Executor::new(self, forge)
    }
}
