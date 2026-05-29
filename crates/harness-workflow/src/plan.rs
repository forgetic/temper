//! Pure queue evaluation and transition planning (Phase 5).
//!
//! This module is the deterministic, side-effect-free state-machine layer. It
//! answers two questions over already-[classified](crate::classify) artifacts:
//!
//! - **Queue matching**: does an artifact belong to a queue?
//! - **Transition planning**: may a role apply a transition to an artifact, and
//!   if so, what typed effects would it produce?
//!
//! Nothing here touches a Forge backend. A [`Planner`] borrows a
//! [`ValidatedWorkflow`] (never a raw spec), reads a classified artifact's kind,
//! labels, and states, and returns either a [`TransitionPlan`] of typed
//! [`WorkflowEffect`]s plus [`Postcondition`]s, or a [`PlanError`] collecting
//! every [`PlanDiagnostic`]. Applying the plan against a Forge backend is a
//! later phase (see `docs/how-to/implement-workflow-layer-in-phases.md`).
//!
//! Queue matching also works against the compiled [`QueueManifest`] through the
//! [`QueueQuery`] trait, so the same logic serves the validated model and a
//! compiled runtime table.

use crate::classify::{ArtifactSource, ClassifiedArtifact};
use crate::compile::QueueManifest;
use crate::ids::{
    ArtifactKindId, GateId, LabelId, QueueId, RoleId, StateDimensionId, StateId, TransitionId,
};
use crate::metadata::Lease;
use crate::validated::{
    Effect, GateCondition, ValidatedQueue, ValidatedTransition, ValidatedWorkflow,
};
use std::collections::HashSet;
use std::error::Error;
use std::fmt;

/// A queue query: an artifact kind plus the labels an artifact must carry.
///
/// Implemented by both [`ValidatedQueue`] and the compiled [`QueueManifest`] so
/// [`matches_queue`] works from the validated model or a compiled manifest.
pub trait QueueQuery {
    /// Artifact kind the queue selects.
    fn queue_artifact(&self) -> &ArtifactKindId;
    /// Labels that must all be present for an artifact to match.
    fn queue_labels(&self) -> &[LabelId];
}

impl QueueQuery for ValidatedQueue {
    fn queue_artifact(&self) -> &ArtifactKindId {
        &self.artifact
    }
    fn queue_labels(&self) -> &[LabelId] {
        &self.labels
    }
}

impl QueueQuery for QueueManifest {
    fn queue_artifact(&self) -> &ArtifactKindId {
        &self.artifact
    }
    fn queue_labels(&self) -> &[LabelId] {
        &self.labels
    }
}

/// Returns `true` when a classified artifact matches a queue query.
///
/// An artifact matches when its kind equals the queue's artifact kind and every
/// label the queue requires is present on the artifact. Because exclusive state
/// dimensions project to mutually exclusive labels, a `code + ready` queue
/// naturally excludes `blocked` or `in-progress` code issues.
pub fn matches_queue<Q: QueueQuery>(query: &Q, artifact: &ClassifiedArtifact) -> bool {
    if query.queue_artifact() != &artifact.kind {
        return false;
    }
    let labels: HashSet<&str> = artifact.labels.iter().map(String::as_str).collect();
    query
        .queue_labels()
        .iter()
        .all(|label| labels.contains(label.as_str()))
}

/// A typed side effect a transition plan would apply.
///
/// This is a closed enum so executors and reconcilers must handle every
/// variant. Effects are relative to the plan's [`TransitionPlan::target`],
/// except the create variants, which request a brand-new artifact.
///
/// Transition specs can produce label, assignee, comment, pull-request create,
/// and merge effects. The executor applies those runtime effects except for the
/// lease placeholders, which are still rejected as unsupported before mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkflowEffect {
    /// Add a label to the target artifact. Produced from an `add_label` effect.
    AddLabel(LabelId),
    /// Remove a label from the target artifact. Produced from a `remove_label`
    /// effect.
    RemoveLabel(LabelId),
    /// Set the target artifact's assignee to the worker/user resolved for this
    /// declared workflow role.
    SetAssignee { role: RoleId },
    /// Remove the assignee resolved for this declared workflow role from the
    /// target artifact.
    RemoveAssignee { role: RoleId },
    /// Post a prose/template comment body on the target artifact.
    CreateComment { body: String },
    /// Request creation of a new issue, keyed for idempotent retries.
    CreateIssue { correlation_key: String },
    /// Request creation of a new pull request. The optional correlation key
    /// identifies retries; branch, title, body, and labels come from runtime
    /// context in a later execution phase.
    CreatePullRequest { correlation_key: Option<String> },
    /// Set or refresh the claim lease on the target artifact.
    ///
    /// Placeholder: leases are modeled in [`crate::metadata`] but no transition
    /// spec emits lease effects yet.
    UpdateLease { lease: Lease },
    /// Clear the claim lease on the target artifact.
    ///
    /// Placeholder (see [`WorkflowEffect::UpdateLease`]).
    ReleaseLease,
    /// Request merging the target pull request. Carries no portable payload.
    MergePullRequest,
}

/// A condition that must hold after a plan's effects are applied.
///
/// Postconditions let an executor verify a transition actually took effect on
/// fresh Forge state. They are derived from the transition's label and assignee
/// effects. Comment effects have no postcondition: a comment is an append-only
/// event, not a queryable state projection, so there is no after-the-fact
/// predicate to assert (the executor instead guarantees a comment is posted
/// at most once through an idempotency marker — see [`crate::execute`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Postcondition {
    /// The label must be present on the target artifact.
    LabelPresent(LabelId),
    /// The label must be absent from the target artifact.
    LabelAbsent(LabelId),
    /// The Forge user resolved for this role must be assigned to the target.
    AssigneePresent { role: RoleId },
    /// The Forge user resolved for this role must not be assigned to the target.
    AssigneeAbsent { role: RoleId },
}

/// A planned, not-yet-executed transition.
///
/// Produced by [`Planner::plan_transition`]. It records what would change
/// (`effects`) and what must hold afterward (`postconditions`) without applying
/// anything. Planning is deterministic: effects and postconditions follow the
/// transition's declared effect order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionPlan {
    /// Transition that was planned.
    pub transition: TransitionId,
    /// Role the plan was authorized for.
    pub role: RoleId,
    /// Artifact kind the transition acts on.
    pub artifact: ArtifactKindId,
    /// Forge artifact the effects target.
    pub target: ArtifactSource,
    /// Typed effects to apply, in declaration order.
    pub effects: Vec<WorkflowEffect>,
    /// Conditions that must hold once the effects are applied.
    pub postconditions: Vec<Postcondition>,
}

/// A single reason a transition cannot be planned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanDiagnostic {
    /// The workflow declares no transition with this id.
    UnknownTransition { transition: TransitionId },
    /// The role is not authorized to perform the transition.
    Unauthorized {
        transition: TransitionId,
        role: RoleId,
    },
    /// The artifact's kind differs from the kind the transition acts on.
    ArtifactKindMismatch {
        transition: TransitionId,
        expected: ArtifactKindId,
        actual: ArtifactKindId,
    },
    /// A label a remove-effect targets is already absent: the source state is
    /// stale, so the transition would do nothing meaningful.
    StalePrecondition {
        transition: TransitionId,
        label: LabelId,
    },
    /// A label an add-effect targets is already present: the transition has
    /// already been applied or contradicts the artifact's current state.
    ContradictedPrecondition {
        transition: TransitionId,
        label: LabelId,
    },
    /// A required gate is not satisfied by the artifact's current labels.
    GateNotSatisfied {
        transition: TransitionId,
        gate: GateId,
    },
    /// Applying the effects would leave an exclusive dimension in several
    /// states at once. Diagnosed before planning so the impossible state never
    /// reaches a Forge backend.
    ImpossibleState {
        transition: TransitionId,
        dimension: StateDimensionId,
        states: Vec<StateId>,
    },
}

impl fmt::Display for PlanDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanDiagnostic::UnknownTransition { transition } => {
                write!(formatter, "no transition `{transition}` is declared")
            }
            PlanDiagnostic::Unauthorized { transition, role } => write!(
                formatter,
                "role `{role}` is not authorized for transition `{transition}`"
            ),
            PlanDiagnostic::ArtifactKindMismatch {
                transition,
                expected,
                actual,
            } => write!(
                formatter,
                "transition `{transition}` acts on `{expected}` but the artifact is `{actual}`"
            ),
            PlanDiagnostic::StalePrecondition { transition, label } => write!(
                formatter,
                "transition `{transition}` removes label `{label}` but it is already absent"
            ),
            PlanDiagnostic::ContradictedPrecondition { transition, label } => write!(
                formatter,
                "transition `{transition}` adds label `{label}` but it is already present"
            ),
            PlanDiagnostic::GateNotSatisfied { transition, gate } => write!(
                formatter,
                "transition `{transition}` requires gate `{gate}`, which is not satisfied"
            ),
            PlanDiagnostic::ImpossibleState {
                transition,
                dimension,
                states,
            } => write!(
                formatter,
                "transition `{transition}` would put exclusive dimension `{dimension}` into states: {}",
                join_states(states)
            ),
        }
    }
}

fn join_states(ids: &[StateId]) -> String {
    ids.iter()
        .map(|id| format!("`{id}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Error returned when a transition cannot be planned.
///
/// Carries every diagnostic found so a caller sees all problems at once,
/// matching the diagnostic-collecting style of validation and classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanError {
    diagnostics: Vec<PlanDiagnostic>,
}

impl PlanError {
    /// Returns the collected diagnostics.
    pub fn diagnostics(&self) -> &[PlanDiagnostic] {
        &self.diagnostics
    }
}

impl fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "transition planning failed with {} diagnostic(s)",
            self.diagnostics.len()
        )?;
        for diagnostic in &self.diagnostics {
            write!(formatter, "\n  - {diagnostic}")?;
        }
        Ok(())
    }
}

impl Error for PlanError {}

/// Pure planner over a validated workflow.
///
/// Bound to a [`ValidatedWorkflow`] so it has the full semantic model — queues,
/// transitions, gates, and state dimensions — that planning needs. It never
/// mutates Forge state.
pub struct Planner<'a> {
    workflow: &'a ValidatedWorkflow,
}

impl<'a> Planner<'a> {
    /// Creates a planner bound to a validated workflow.
    pub fn new(workflow: &'a ValidatedWorkflow) -> Self {
        Self { workflow }
    }

    /// Returns the ids of every queue the artifact matches, in declaration
    /// order.
    pub fn matching_queues(&self, artifact: &ClassifiedArtifact) -> Vec<QueueId> {
        self.workflow
            .queues()
            .iter()
            .filter(|queue| matches_queue(*queue, artifact))
            .map(|queue| queue.id.clone())
            .collect()
    }

    /// Returns the artifacts that match the given queue, preserving input order.
    ///
    /// An unknown queue id yields an empty selection.
    pub fn queue_members<'c>(
        &self,
        queue: &QueueId,
        artifacts: &'c [ClassifiedArtifact],
    ) -> Vec<&'c ClassifiedArtifact> {
        let Some(query) = self.workflow.queues().iter().find(|q| &q.id == queue) else {
            return Vec::new();
        };
        artifacts
            .iter()
            .filter(|artifact| matches_queue(query, artifact))
            .collect()
    }

    /// Plans a transition for a role against a classified artifact.
    ///
    /// Returns a [`TransitionPlan`] when the role is authorized, the artifact
    /// kind matches, all label preconditions hold, every required gate is
    /// satisfied, and the result would not create an impossible exclusive state.
    /// Otherwise returns a [`PlanError`] collecting every problem.
    pub fn plan_transition(
        &self,
        transition: &TransitionId,
        role: &RoleId,
        artifact: &ClassifiedArtifact,
    ) -> Result<TransitionPlan, PlanError> {
        let mut diagnostics = Vec::new();

        let Some(declared) = self
            .workflow
            .transitions()
            .iter()
            .find(|candidate| &candidate.id == transition)
        else {
            return Err(PlanError {
                diagnostics: vec![PlanDiagnostic::UnknownTransition {
                    transition: transition.clone(),
                }],
            });
        };

        if !declared.roles.contains(role) {
            diagnostics.push(PlanDiagnostic::Unauthorized {
                transition: declared.id.clone(),
                role: role.clone(),
            });
        }

        // Label, gate, and state checks only make sense when the artifact is the
        // kind the transition acts on; otherwise they would compare against the
        // wrong labels.
        if declared.artifact == artifact.kind {
            let labels: HashSet<&str> = artifact.labels.iter().map(String::as_str).collect();
            self.check_preconditions(declared, &labels, &mut diagnostics);
            self.check_gates(declared, artifact, &labels, &mut diagnostics);
            self.check_resulting_states(declared, &labels, &mut diagnostics);
        } else {
            diagnostics.push(PlanDiagnostic::ArtifactKindMismatch {
                transition: declared.id.clone(),
                expected: declared.artifact.clone(),
                actual: artifact.kind.clone(),
            });
        }

        if diagnostics.is_empty() {
            Ok(TransitionPlan {
                transition: declared.id.clone(),
                role: role.clone(),
                artifact: declared.artifact.clone(),
                target: artifact.source,
                effects: declared.effects.iter().map(to_effect).collect(),
                postconditions: declared
                    .effects
                    .iter()
                    .filter_map(to_postcondition)
                    .collect(),
            })
        } else {
            Err(PlanError { diagnostics })
        }
    }

    /// Checks each effect's label precondition against current labels.
    fn check_preconditions(
        &self,
        transition: &ValidatedTransition,
        labels: &HashSet<&str>,
        diagnostics: &mut Vec<PlanDiagnostic>,
    ) {
        for effect in &transition.effects {
            match effect {
                Effect::RemoveLabel(label) if !labels.contains(label.as_str()) => {
                    diagnostics.push(PlanDiagnostic::StalePrecondition {
                        transition: transition.id.clone(),
                        label: label.clone(),
                    });
                }
                Effect::AddLabel(label) if labels.contains(label.as_str()) => {
                    diagnostics.push(PlanDiagnostic::ContradictedPrecondition {
                        transition: transition.id.clone(),
                        label: label.clone(),
                    });
                }
                _ => {}
            }
        }
    }

    /// Checks that every required gate is satisfied by current labels.
    fn check_gates(
        &self,
        transition: &ValidatedTransition,
        artifact: &ClassifiedArtifact,
        labels: &HashSet<&str>,
        diagnostics: &mut Vec<PlanDiagnostic>,
    ) {
        for gate in &transition.requires_gates {
            if !self.gate_satisfied(gate, artifact, labels) {
                diagnostics.push(PlanDiagnostic::GateNotSatisfied {
                    transition: transition.id.clone(),
                    gate: gate.clone(),
                });
            }
        }
    }

    /// A gate is satisfied when its external condition holds or some
    /// satisfying transition's added labels are all present.
    fn gate_satisfied(
        &self,
        gate: &GateId,
        artifact: &ClassifiedArtifact,
        labels: &HashSet<&str>,
    ) -> bool {
        let Some(declared) = self.workflow.gates().iter().find(|g| &g.id == gate) else {
            return false;
        };
        declared
            .condition
            .as_ref()
            .is_some_and(|condition| gate_condition_satisfied(condition, artifact, labels))
            || declared.satisfied_by.iter().any(|transition_id| {
                let Some(transition) = self
                    .workflow
                    .transitions()
                    .iter()
                    .find(|t| &t.id == transition_id)
                else {
                    return false;
                };
                let mut produced = transition.effects.iter().filter_map(|effect| match effect {
                    Effect::AddLabel(label) => Some(label),
                    Effect::RemoveLabel(_)
                    | Effect::SetAssignee(_)
                    | Effect::RemoveAssignee(_)
                    | Effect::CreateComment { .. }
                    | Effect::CreatePullRequest { .. }
                    | Effect::MergePullRequest => None,
                });
                let mut any = false;
                let all_present = produced.all(|label| {
                    any = true;
                    labels.contains(label.as_str())
                });
                any && all_present
            })
    }

    /// Diagnoses exclusive dimensions that would hold several states after the
    /// effects are applied.
    fn check_resulting_states(
        &self,
        transition: &ValidatedTransition,
        labels: &HashSet<&str>,
        diagnostics: &mut Vec<PlanDiagnostic>,
    ) {
        let mut result: HashSet<String> = labels.iter().map(|label| label.to_string()).collect();
        for effect in &transition.effects {
            match effect {
                Effect::AddLabel(label) => {
                    result.insert(label.as_str().to_string());
                }
                Effect::RemoveLabel(label) => {
                    result.remove(label.as_str());
                }
                Effect::SetAssignee(_)
                | Effect::RemoveAssignee(_)
                | Effect::CreateComment { .. }
                | Effect::CreatePullRequest { .. }
                | Effect::MergePullRequest => {}
            }
        }

        for dimension in self.workflow.state_dimensions() {
            if !dimension.exclusive {
                continue;
            }
            let active: Vec<StateId> = dimension
                .states
                .iter()
                .filter(|state| {
                    state
                        .label
                        .as_ref()
                        .is_some_and(|label| result.contains(label.as_str()))
                })
                .map(|state| state.id.clone())
                .collect();
            if active.len() > 1 {
                diagnostics.push(PlanDiagnostic::ImpossibleState {
                    transition: transition.id.clone(),
                    dimension: dimension.id.clone(),
                    states: active,
                });
            }
        }
    }
}

fn gate_condition_satisfied(
    condition: &GateCondition,
    artifact: &ClassifiedArtifact,
    labels: &HashSet<&str>,
) -> bool {
    match condition {
        GateCondition::LabelPresent(label) => labels.contains(label.as_str()),
        GateCondition::StateEquals { dimension, state } => artifact
            .states
            .get(dimension)
            .is_some_and(|states| states.contains(state)),
    }
}

impl ValidatedWorkflow {
    /// Returns a [`Planner`] bound to this workflow.
    ///
    /// Convenience wrapper around [`Planner::new`].
    pub fn planner(&self) -> Planner<'_> {
        Planner::new(self)
    }
}

/// Maps a declarative transition effect into a planning effect.
fn to_effect(effect: &Effect) -> WorkflowEffect {
    match effect {
        Effect::AddLabel(label) => WorkflowEffect::AddLabel(label.clone()),
        Effect::RemoveLabel(label) => WorkflowEffect::RemoveLabel(label.clone()),
        Effect::SetAssignee(role) => WorkflowEffect::SetAssignee { role: role.clone() },
        Effect::RemoveAssignee(role) => WorkflowEffect::RemoveAssignee { role: role.clone() },
        Effect::CreateComment { body } => WorkflowEffect::CreateComment { body: body.clone() },
        Effect::CreatePullRequest { correlation_key } => WorkflowEffect::CreatePullRequest {
            correlation_key: correlation_key.clone(),
        },
        Effect::MergePullRequest => WorkflowEffect::MergePullRequest,
    }
}

/// Derives the postcondition implied by a transition effect, if any.
fn to_postcondition(effect: &Effect) -> Option<Postcondition> {
    match effect {
        Effect::AddLabel(label) => Some(Postcondition::LabelPresent(label.clone())),
        Effect::RemoveLabel(label) => Some(Postcondition::LabelAbsent(label.clone())),
        Effect::SetAssignee(role) => Some(Postcondition::AssigneePresent { role: role.clone() }),
        Effect::RemoveAssignee(role) => Some(Postcondition::AssigneeAbsent { role: role.clone() }),
        Effect::CreateComment { .. }
        | Effect::CreatePullRequest { .. }
        | Effect::MergePullRequest => None,
    }
}
