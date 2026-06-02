//! Compilation of a validated workflow into agent- and runtime-facing manifests.
//!
//! Compilation is the bridge between a [`ValidatedWorkflow`] and the consumers
//! that act on it: agent runners receive a per-role prompt and a narrow,
//! intent-level tool surface; runtime setup receives queue and label manifests
//! and a transition table. Compilation never calls a Forge backend, generates
//! tool bodies, or executes transitions; it only projects the validated model
//! into the shapes those consumers need.
//!
//! The entry point is [`compile`] (also available as
//! [`ValidatedWorkflow::compile`]). Because it accepts only a
//! [`ValidatedWorkflow`], duplicate ids and undeclared references are already
//! ruled out, and a role's tool surface is derived from the transitions it is
//! authorized for, so a role can never see a tool it is not allowed to use.
//!
//! All outputs are deterministic: they follow the declaration order of the
//! validated workflow, so prompts and manifests are stable enough for
//! snapshot-style assertions.

use crate::ids::{
    ArtifactKindId, GateId, LabelId, QueueId, RoleId, StateDimensionId, StateId, TransitionId,
};
use crate::prompt::build_prompt;
use crate::validated::{
    Effect, GateCondition, QueueLabelSet, RolePromptExtension, ValidatedRole, ValidatedTransition,
    ValidatedWorkflow,
};
use chrono::Duration;

pub use crate::prompt::{PromptManifest, PromptSection};

/// A validated workflow projected into manifests for agents and runtime setup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledWorkflow {
    name: String,
    roles: Vec<RoleManifest>,
    queues: Vec<QueueManifest>,
    transitions: Vec<TransitionManifest>,
    labels: LabelManifest,
}

impl CompiledWorkflow {
    /// Returns the workflow name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the compiled role manifests, in declaration order.
    pub fn roles(&self) -> &[RoleManifest] {
        &self.roles
    }

    /// Returns the role manifest with the given id, if present.
    pub fn role(&self, id: &RoleId) -> Option<&RoleManifest> {
        self.roles.iter().find(|role| &role.id == id)
    }

    /// Returns the queue manifests for runtime queue evaluation.
    pub fn queues(&self) -> &[QueueManifest] {
        &self.queues
    }

    /// Returns the runtime transition table.
    pub fn transitions(&self) -> &[TransitionManifest] {
        &self.transitions
    }

    /// Returns the label manifest used to set up Forge labels.
    pub fn labels(&self) -> &LabelManifest {
        &self.labels
    }
}

/// Everything a single role's agent runner needs: identity, user guidance,
/// concurrency hint, subscribed queues, transition authority, the intent-level
/// tools that authority compiles into, and a deterministic prompt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleManifest {
    pub id: RoleId,
    /// Legacy user guidance carried from `RawRole::charter`.
    pub charter: Option<String>,
    /// Structured user-authored prompt extension for this role.
    pub prompt_extension: RolePromptExtension,
    /// How many artifacts the role may hold at once, or `None` for no limit.
    pub concurrency: Option<u32>,
    /// Queues the role draws work from.
    pub queues: Vec<QueueId>,
    /// Transitions the role is authorized to perform.
    pub authority: Vec<TransitionId>,
    /// Intent-level tools, one per authorized transition.
    pub tools: Vec<ToolManifest>,
    /// Deterministic prompt sections for the role.
    pub prompt: PromptManifest,
}

/// An intent-level operation exposed to a role.
///
/// A tool maps one-to-one to a transition the role is authorized for. It
/// carries the transition's artifact kind, required gates, and effects so a
/// later phase can generate a body that enforces preconditions and applies only
/// the authorized effects. No generic Forge mutation is exposed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolManifest {
    /// Intent-level tool name; equal to the transition id (e.g. `claim_code`).
    pub name: String,
    /// Transition this tool applies.
    pub transition: TransitionId,
    /// Artifact kind the tool operates on.
    pub artifact: ArtifactKindId,
    /// Gates that must be satisfied before the tool may run.
    pub requires_gates: Vec<GateId>,
    /// Effects the tool applies when it runs.
    pub effects: Vec<Effect>,
}

/// A queue projected for runtime evaluation, with filters, subscribers, and activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueManifest {
    pub id: QueueId,
    pub artifacts: Vec<ArtifactKindId>,
    pub labels: Vec<LabelId>,
    pub any_of: Vec<QueueLabelSet>,
    pub min_depth: Option<u32>,
    pub max_age: Option<Duration>,
    pub condition: Option<GateCondition>,
    /// Roles that draw work from this queue, in role declaration order.
    pub subscribers: Vec<RoleId>,
}

/// One row of the runtime transition table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionManifest {
    pub id: TransitionId,
    pub artifact: ArtifactKindId,
    pub roles: Vec<RoleId>,
    pub requires_gates: Vec<GateId>,
    pub effects: Vec<Effect>,
}

/// The labels a workflow needs, each annotated with why it is needed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LabelManifest {
    labels: Vec<LabelSpec>,
}

impl LabelManifest {
    /// Returns the label specs in workflow declaration order.
    pub fn labels(&self) -> &[LabelSpec] {
        &self.labels
    }

    /// Returns the spec for a label id, if the workflow declares it.
    pub fn get(&self, id: &LabelId) -> Option<&LabelSpec> {
        self.labels.iter().find(|spec| &spec.id == id)
    }

    /// Returns `true` when the manifest contains the given label id.
    pub fn contains(&self, id: &LabelId) -> bool {
        self.get(id).is_some()
    }
}

/// A single label and the workflow sites that reference it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LabelSpec {
    pub id: LabelId,
    /// Why the label is needed. Empty for a declared-but-unreferenced label.
    pub usages: Vec<LabelUsage>,
}

/// A site that requires a label to exist in the Forge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LabelUsage {
    /// The label identifies an artifact kind.
    ArtifactIdentity { artifact: ArtifactKindId },
    /// The label projects a state of a dimension.
    StateProjection {
        dimension: StateDimensionId,
        state: StateId,
    },
    /// The label is part of a queue's filter.
    QueueFilter { queue: QueueId },
    /// A transition effect adds or removes the label.
    TransitionEffect { transition: TransitionId },
    /// The label is produced by a transition that satisfies a gate.
    GateOutcome { gate: GateId },
    /// The label is a Forge-projected condition for an external gate.
    GateCondition { gate: GateId },
}

/// Compiles a validated workflow into manifests for agents and runtime setup.
///
/// Compilation cannot fail: a [`ValidatedWorkflow`] is already internally
/// consistent, and every output is derived from it.
pub fn compile(workflow: &ValidatedWorkflow) -> CompiledWorkflow {
    let transitions = compile_transitions(workflow);
    let queues = compile_queues(workflow);
    let labels = compile_labels(workflow);
    let roles = workflow
        .roles()
        .iter()
        .map(|role| compile_role(workflow, role, &queues))
        .collect();

    CompiledWorkflow {
        name: workflow.name().to_string(),
        roles,
        queues,
        transitions,
        labels,
    }
}

impl ValidatedWorkflow {
    /// Compiles this workflow into agent- and runtime-facing manifests.
    ///
    /// Convenience wrapper around [`compile`].
    pub fn compile(&self) -> CompiledWorkflow {
        compile(self)
    }
}

fn compile_transitions(workflow: &ValidatedWorkflow) -> Vec<TransitionManifest> {
    workflow
        .transitions()
        .iter()
        .map(|transition| TransitionManifest {
            id: transition.id.clone(),
            artifact: transition.artifact.clone(),
            roles: transition.roles.clone(),
            requires_gates: transition.requires_gates.clone(),
            effects: transition.effects.clone(),
        })
        .collect()
}

fn compile_queues(workflow: &ValidatedWorkflow) -> Vec<QueueManifest> {
    workflow
        .queues()
        .iter()
        .map(|queue| QueueManifest {
            id: queue.id.clone(),
            artifacts: queue.artifacts.clone(),
            labels: queue.labels.clone(),
            any_of: queue.any_of.clone(),
            min_depth: queue.min_depth,
            max_age: queue.max_age,
            condition: queue.condition.clone(),
            subscribers: workflow
                .roles()
                .iter()
                .filter(|role| role.queues.contains(&queue.id))
                .map(|role| role.id.clone())
                .collect(),
        })
        .collect()
}

fn compile_role(
    workflow: &ValidatedWorkflow,
    role: &ValidatedRole,
    queues: &[QueueManifest],
) -> RoleManifest {
    let authorized: Vec<&ValidatedTransition> = workflow
        .transitions()
        .iter()
        .filter(|transition| transition.roles.contains(&role.id))
        .collect();

    let authority = authorized.iter().map(|t| t.id.clone()).collect();
    let tools: Vec<ToolManifest> = authorized
        .iter()
        .map(|transition| ToolManifest {
            name: transition.id.to_string(),
            transition: transition.id.clone(),
            artifact: transition.artifact.clone(),
            requires_gates: transition.requires_gates.clone(),
            effects: transition.effects.clone(),
        })
        .collect();

    let prompt = build_prompt(workflow.name(), role, queues, &tools);

    RoleManifest {
        id: role.id.clone(),
        charter: role.charter.clone(),
        prompt_extension: role.prompt.clone(),
        concurrency: role.concurrency,
        queues: role.queues.clone(),
        authority,
        tools,
        prompt,
    }
}

fn compile_labels(workflow: &ValidatedWorkflow) -> LabelManifest {
    let mut specs: Vec<LabelSpec> = workflow
        .labels()
        .iter()
        .map(|id| LabelSpec {
            id: id.clone(),
            usages: Vec::new(),
        })
        .collect();

    let mut record = |label: &LabelId, usage: LabelUsage| {
        if let Some(spec) = specs.iter_mut().find(|spec| &spec.id == label) {
            if !spec.usages.contains(&usage) {
                spec.usages.push(usage);
            }
        }
    };

    for kind in workflow.artifact_kinds() {
        for label in &kind.identifying_labels {
            record(
                label,
                LabelUsage::ArtifactIdentity {
                    artifact: kind.id.clone(),
                },
            );
        }
    }

    for dimension in workflow.state_dimensions() {
        for state in &dimension.states {
            if let Some(label) = &state.label {
                record(
                    label,
                    LabelUsage::StateProjection {
                        dimension: dimension.id.clone(),
                        state: state.id.clone(),
                    },
                );
            }
        }
    }

    for queue in workflow.queues() {
        for label in &queue.labels {
            record(
                label,
                LabelUsage::QueueFilter {
                    queue: queue.id.clone(),
                },
            );
        }
        for label_set in &queue.any_of {
            for label in &label_set.labels {
                record(
                    label,
                    LabelUsage::QueueFilter {
                        queue: queue.id.clone(),
                    },
                );
            }
        }
        if let Some(condition) = &queue.condition {
            if let Some(label) = gate_condition_label(condition, workflow) {
                record(
                    label,
                    LabelUsage::QueueFilter {
                        queue: queue.id.clone(),
                    },
                );
            }
        }
    }

    for transition in workflow.transitions() {
        for effect in &transition.effects {
            if let Some(label) = effect_label(effect) {
                record(
                    label,
                    LabelUsage::TransitionEffect {
                        transition: transition.id.clone(),
                    },
                );
            }
        }
    }

    for gate in workflow.gates() {
        if let Some(condition) = &gate.condition {
            if let Some(label) = gate_condition_label(condition, workflow) {
                record(
                    label,
                    LabelUsage::GateCondition {
                        gate: gate.id.clone(),
                    },
                );
            }
        }
        for transition_id in &gate.satisfied_by {
            let Some(transition) = workflow
                .transitions()
                .iter()
                .find(|t| &t.id == transition_id)
            else {
                continue;
            };
            for effect in &transition.effects {
                if let Some(label) = effect_label(effect) {
                    record(
                        label,
                        LabelUsage::GateOutcome {
                            gate: gate.id.clone(),
                        },
                    );
                }
            }
        }
    }

    LabelManifest { labels: specs }
}

fn gate_condition_label<'a>(
    condition: &'a GateCondition,
    workflow: &'a ValidatedWorkflow,
) -> Option<&'a LabelId> {
    match condition {
        GateCondition::LabelPresent(label) => Some(label),
        GateCondition::StateEquals { dimension, state } => workflow
            .state_dimensions()
            .iter()
            .find(|candidate| &candidate.id == dimension)?
            .states
            .iter()
            .find(|candidate| &candidate.id == state)?
            .label
            .as_ref(),
        // Reads `dependency` relations by kind, not a label, so it tracks no
        // label usage.
        GateCondition::DependenciesResolved => None,
        // Reads native CI conclusions, not a label, so it tracks no label
        // usage (see ADR 0014).
        GateCondition::CiPassed | GateCondition::CiFailed => None,
        GateCondition::ReviewApproved | GateCondition::ReviewChangesRequested => None,
    }
}

fn effect_label(effect: &Effect) -> Option<&LabelId> {
    match effect {
        Effect::AddLabel(label) | Effect::RemoveLabel(label) => Some(label),
        Effect::SetAssignee(_)
        | Effect::RemoveAssignee(_)
        | Effect::CreateComment { .. }
        | Effect::CreatePullRequest { .. }
        | Effect::RequestReviewers { .. }
        | Effect::SubmitReview { .. }
        | Effect::MergePullRequest => None,
    }
}
