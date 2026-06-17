//! The pure [`Planner`] over a validated workflow.
//!
//! Bound to a [`ValidatedWorkflow`], it answers queue-matching and
//! transition-planning questions and produces typed [`TransitionPlan`]s and
//! [`MechanicalPlan`]s without ever touching a Forge backend. Split from the
//! planning root to keep each file within the source-size budget.

use super::conditions::gate_condition_satisfied;
use super::dependency::{DependencyStatus, MechanicalPlan};
use super::diagnostic::{PlanDiagnostic, PlanError};
use super::queue::{matches_queue, matches_queue_with};
use super::signals::GateSignals;
use super::state;
use super::types::{Postcondition, TransitionPlan, WorkflowEffect};
use crate::classify::ClassifiedArtifact;
use crate::ids::{GateId, QueueId, RoleId, TransitionId};
use crate::relation::RelationKind;
use crate::validated::{Effect, GateCondition, ValidatedTransition, ValidatedWorkflow};
use chrono::{DateTime, Utc};
use std::collections::HashSet;

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
        self.matching_queues_with(artifact, &GateSignals::default())
    }

    /// Returns matching queues using runtime-fed queue conditions.
    pub fn matching_queues_with(
        &self,
        artifact: &ClassifiedArtifact,
        signals: &GateSignals,
    ) -> Vec<QueueId> {
        self.workflow
            .queues()
            .iter()
            .filter(|queue| matches_queue_with(*queue, artifact, signals))
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

    /// Returns queue members using runtime-fed queue conditions.
    pub fn queue_members_with<'c>(
        &self,
        queue: &QueueId,
        artifacts: &'c [ClassifiedArtifact],
        signals: &GateSignals,
    ) -> Vec<&'c ClassifiedArtifact> {
        let Some(query) = self.workflow.queues().iter().find(|q| &q.id == queue) else {
            return Vec::new();
        };
        artifacts
            .iter()
            .filter(|artifact| matches_queue_with(query, artifact, signals))
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
        super::queue::queue_active(query, &members, now)
    }

    /// Plans a transition for a role against a classified artifact.
    ///
    /// Returns a [`TransitionPlan`] when the role is authorized, the artifact
    /// kind matches, all label preconditions hold, every required gate is
    /// satisfied, and the result would not create an impossible exclusive state.
    /// Otherwise returns a [`PlanError`] collecting every problem.
    ///
    /// Gate signals are empty, so dependency gates are open only for artifacts
    /// with no dependency relations and CI gates stay closed. Use
    /// [`Planner::plan_transition_with`] when the runtime knows which
    /// prerequisites have landed and the current CI aggregate.
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
            return Err(PlanError::new(vec![PlanDiagnostic::UnknownTransition {
                transition: transition.clone(),
            }]));
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
            state::check_resulting_states(
                self.workflow,
                declared,
                artifact,
                &labels,
                &mut diagnostics,
            );
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
            Err(PlanError::new(diagnostics))
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
                Effect::RemoveLabelIfPresent(_) => {}
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
    /// `blocked` once every prerequisite has landed.
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
        // here; bundle the dependency status with a default pending CI.
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
                state::check_resulting_states(
                    self.workflow,
                    transition,
                    artifact,
                    &labels,
                    &mut diagnostics,
                );
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
                    | Effect::RemoveLabelIfPresent(_)
                    | Effect::SetAssignee(_)
                    | Effect::RemoveAssignee(_)
                    | Effect::CreateComment { .. }
                    | Effect::CreatePullRequest { .. }
                    | Effect::RequestReviewers { .. }
                    | Effect::SubmitReview { .. }
                    | Effect::SetBody { .. }
                    | Effect::AttachReview { .. }
                    | Effect::CreateIssues { .. }
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
        Effect::RemoveLabel(label) | Effect::RemoveLabelIfPresent(label) => {
            WorkflowEffect::RemoveLabel(label.clone())
        }
        Effect::SetAssignee(role) => WorkflowEffect::SetAssignee { role: role.clone() },
        Effect::RemoveAssignee(role) => WorkflowEffect::RemoveAssignee { role: role.clone() },
        Effect::CreateComment { body } => WorkflowEffect::CreateComment { body: body.clone() },
        Effect::CreatePullRequest { correlation_key } => WorkflowEffect::CreatePullRequest {
            correlation_key: correlation_key.clone(),
        },
        Effect::RequestReviewers { roles } => WorkflowEffect::RequestReviewers {
            roles: roles.clone(),
        },
        Effect::SubmitReview { decision } => WorkflowEffect::SubmitReview {
            decision: *decision,
        },
        Effect::SetBody { correlation_key } => WorkflowEffect::SetBody {
            correlation_key: correlation_key.clone(),
        },
        Effect::AttachReview {
            decision,
            correlation_key,
        } => WorkflowEffect::AttachReview {
            decision: *decision,
            correlation_key: correlation_key.clone(),
        },
        Effect::CreateIssues { correlation_key } => WorkflowEffect::CreateIssues {
            correlation_key: correlation_key.clone(),
        },
        Effect::MergePullRequest => WorkflowEffect::MergePullRequest,
    }
}

/// Derives the postcondition implied by a transition effect, if any.
fn to_postcondition(effect: &Effect) -> Option<Postcondition> {
    match effect {
        Effect::AddLabel(label) => Some(Postcondition::LabelPresent(label.clone())),
        Effect::RemoveLabel(label) | Effect::RemoveLabelIfPresent(label) => {
            Some(Postcondition::LabelAbsent(label.clone()))
        }
        Effect::SetAssignee(role) => Some(Postcondition::AssigneePresent { role: role.clone() }),
        Effect::RemoveAssignee(role) => Some(Postcondition::AssigneeAbsent { role: role.clone() }),
        Effect::CreateComment { .. }
        | Effect::CreatePullRequest { .. }
        | Effect::RequestReviewers { .. }
        | Effect::SubmitReview { .. }
        | Effect::SetBody { .. }
        | Effect::AttachReview { .. }
        | Effect::CreateIssues { .. }
        | Effect::MergePullRequest => None,
    }
}
