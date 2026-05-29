//! Pure queue evaluation and transition planning (Phase 5).
//!
//! This module is the deterministic, side-effect-free state-machine layer. It
//! answers three questions over already-[classified](crate::classify) artifacts:
//!
//! - **Queue matching**: does an artifact belong to a queue?
//! - **Queue activation**: should a matched queue be serviced now?
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
//! Queue matching and activation also work against the compiled
//! [`QueueManifest`](crate::compile::QueueManifest) through the [`QueueQuery`]
//! trait, so the same logic serves the validated model and a compiled runtime
//! table.

mod dependency;
mod queue;
mod signals;
mod types;

use crate::classify::ClassifiedArtifact;
use crate::ids::{
    ArtifactKindId, GateId, LabelId, QueueId, RoleId, StateDimensionId, StateId, TransitionId,
};
use crate::relation::RelationKind;
use crate::validated::{Effect, GateCondition, ValidatedTransition, ValidatedWorkflow};
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::error::Error;
use std::fmt;

pub use dependency::{DependencyStatus, MechanicalPlan};
pub use queue::{matches_queue, queue_active, QueueMember, QueueQuery};
pub use signals::{CiStatus, GateSignals};
pub use types::{Postcondition, TransitionPlan, WorkflowEffect};

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

    /// Returns whether a queue's current matched members make it active.
    ///
    /// Matching remains separate: this method first selects members with the
    /// unchanged queue matcher, then applies the queue's optional activation
    /// policy. An unknown queue id is inactive.
    pub fn queue_active(
        &self,
        queue: &QueueId,
        artifacts: &[ClassifiedArtifact],
        now: DateTime<Utc>,
    ) -> bool {
        let Some(query) = self.workflow.queues().iter().find(|q| &q.id == queue) else {
            return false;
        };
        let members: Vec<&ClassifiedArtifact> = artifacts
            .iter()
            .filter(|artifact| matches_queue(query, artifact))
            .collect();
        queue::queue_active(query, &members, now)
    }

    /// Plans a transition for a role against a classified artifact.
    ///
    /// Returns a [`TransitionPlan`] when the role is authorized, the artifact
    /// kind matches, all label preconditions hold, every required gate is
    /// satisfied, and the result would not create an impossible exclusive state.
    /// Otherwise returns a [`PlanError`] collecting every problem.
    ///
    /// Gate signals are empty, so dependency gates are open only for artifacts
    /// with no dependency relations and `ci_passed` gates stay closed. Use
    /// [`Planner::plan_transition_with`] when the runtime knows which
    /// prerequisites have landed and whether CI passed.
    pub fn plan_transition(
        &self,
        transition: &TransitionId,
        role: &RoleId,
        artifact: &ClassifiedArtifact,
    ) -> Result<TransitionPlan, PlanError> {
        self.plan_transition_with(transition, role, artifact, &GateSignals::default())
    }

    /// Plans a transition like [`Planner::plan_transition`], but evaluates
    /// runtime-fed gate conditions against the supplied [`GateSignals`].
    pub fn plan_transition_with(
        &self,
        transition: &TransitionId,
        role: &RoleId,
        artifact: &ClassifiedArtifact,
        signals: &GateSignals,
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
            self.check_gates(declared, artifact, &labels, signals, &mut diagnostics);
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

    /// Returns the mechanical (actor-less) unblock plans an artifact admits
    /// under the given dependency status.
    ///
    /// A plan is returned for each transition that (a) acts on the artifact's
    /// kind, (b) requires a `DependenciesResolved` gate, and (c) has all its
    /// label preconditions, gates, and resulting-state checks satisfied. The
    /// artifact must declare at least one `dependency` relation, so a blocked
    /// artifact with no recorded dependency is never auto-unblocked even though
    /// the gate would be vacuously satisfied. The reconciler uses this to clear
    /// `blocked-on-dependency` once every prerequisite has landed.
    pub fn dependency_unblocks(
        &self,
        artifact: &ClassifiedArtifact,
        deps: &DependencyStatus,
    ) -> Vec<MechanicalPlan> {
        if !artifact
            .relations
            .iter()
            .any(|relation| relation.kind == RelationKind::Dependency)
        {
            return Vec::new();
        }
        let labels: HashSet<&str> = artifact.labels.iter().map(String::as_str).collect();
        // Mechanical unblock is a dependency-gate concern, so CI is irrelevant
        // here; bundle the dependency status with a default (not-passed) CI.
        let signals = GateSignals::new().with_dependencies(deps.clone());
        self.workflow
            .transitions()
            .iter()
            .filter(|transition| transition.artifact == artifact.kind)
            .filter(|transition| self.requires_dependency_gate(transition))
            .filter_map(|transition| {
                let mut diagnostics = Vec::new();
                self.check_preconditions(transition, &labels, &mut diagnostics);
                self.check_gates(transition, artifact, &labels, &signals, &mut diagnostics);
                self.check_resulting_states(transition, &labels, &mut diagnostics);
                diagnostics.is_empty().then(|| MechanicalPlan {
                    transition: transition.id.clone(),
                    target: artifact.source,
                    effects: transition.effects.iter().map(to_effect).collect(),
                    postconditions: transition
                        .effects
                        .iter()
                        .filter_map(to_postcondition)
                        .collect(),
                })
            })
            .collect()
    }

    /// Returns whether the transition requires a `DependenciesResolved` gate.
    fn requires_dependency_gate(&self, transition: &ValidatedTransition) -> bool {
        transition.requires_gates.iter().any(|gate_id| {
            self.workflow.gates().iter().any(|gate| {
                &gate.id == gate_id
                    && matches!(gate.condition, Some(GateCondition::DependenciesResolved))
            })
        })
    }

    /// Checks that every required gate is satisfied by current labels.
    fn check_gates(
        &self,
        transition: &ValidatedTransition,
        artifact: &ClassifiedArtifact,
        labels: &HashSet<&str>,
        signals: &GateSignals,
        diagnostics: &mut Vec<PlanDiagnostic>,
    ) {
        for gate in &transition.requires_gates {
            if !self.gate_satisfied(gate, artifact, labels, signals) {
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
        signals: &GateSignals,
    ) -> bool {
        let Some(declared) = self.workflow.gates().iter().find(|g| &g.id == gate) else {
            return false;
        };
        declared
            .condition
            .as_ref()
            .is_some_and(|condition| gate_condition_satisfied(condition, artifact, labels, signals))
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
    signals: &GateSignals,
) -> bool {
    match condition {
        GateCondition::LabelPresent(label) => labels.contains(label.as_str()),
        GateCondition::StateEquals { dimension, state } => artifact
            .states
            .get(dimension)
            .is_some_and(|states| states.contains(state)),
        GateCondition::DependenciesResolved => artifact
            .relations
            .iter()
            .filter(|relation| relation.kind == RelationKind::Dependency)
            .all(|relation| signals.dependencies().is_landed(relation.target)),
        GateCondition::CiPassed => signals.ci().is_passed(),
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
