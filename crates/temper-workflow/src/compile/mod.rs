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

mod labels;

use crate::ids::{
    ArtifactKindId, ExternalToolId, GateId, LabelId, QueueId, RoleId, TransitionId, VerdictId,
};
use crate::plan::SignalNeeds;
use crate::prompt::build_prompt;
use crate::validated::{
    Effect, GateCondition, QueueAutomation, QueueLabelSet, RolePromptExtension, ValidatedRole,
    ValidatedTransition, ValidatedWorkflow,
};
use chrono::Duration;
use labels::compile_labels;
use std::collections::BTreeMap;

pub use crate::prompt::{PromptManifest, PromptSection};
pub use labels::{LabelManifest, LabelSpec, LabelUsage};

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

    /// Returns the runtime signals needed by the queues a role subscribes to.
    ///
    /// Unknown roles have no subscribed queues and therefore need no signals.
    pub fn signal_needs_for_role(&self, id: &RoleId) -> SignalNeeds {
        let Some(role) = self.role(id) else {
            return SignalNeeds::none();
        };
        self.queues
            .iter()
            .filter(|queue| role.queues.contains(&queue.id))
            .fold(SignalNeeds::none(), |needs, queue| {
                needs.union(SignalNeeds::for_queue(queue))
            })
    }
}

/// Everything a single role's agent runner needs: identity, user guidance,
/// concurrency hint, subscribed queues, transition authority, the intent-level
/// tools that authority compiles into, and a deterministic prompt.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
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
    /// Intent-level workflow tools, one per authorized transition.
    pub tools: Vec<ToolManifest>,
    /// User-declared non-workflow tools, one per role declaration.
    pub external_tools: Vec<ExternalToolManifest>,
    /// Deterministic prompt sections for the role.
    pub prompt: PromptManifest,
}

/// An intent-level operation exposed to a role.
///
/// A tool maps one-to-one to a transition the role is authorized for. It
/// carries the transition's artifact kind, required gates, and effects so a
/// later phase can generate a body that enforces preconditions and applies only
/// the authorized effects. No generic Forge mutation is exposed.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
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
    /// Verdict id -> transition id routing for a workspace-backed action. When
    /// the action's workspace returns a verdict, the runner runs the mapped
    /// transition instead of this tool's transition. Empty unless declared.
    #[serde(default)]
    pub outcomes: BTreeMap<VerdictId, TransitionId>,
}

/// User-declared non-workflow tool metadata.
///
/// These entries grant no executable authority by themselves. A runner must bind
/// a matching provider before an LLM role worker may see the tool as available.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ExternalToolManifest {
    pub id: ExternalToolId,
    pub description: String,
    pub required: bool,
    pub constraints: Vec<String>,
    pub guidance: Option<String>,
}

/// A queue projected for runtime evaluation, with filters, subscribers, activation, and automation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueManifest {
    pub id: QueueId,
    pub artifacts: Vec<ArtifactKindId>,
    pub labels: Vec<LabelId>,
    pub any_of: Vec<QueueLabelSet>,
    pub min_depth: Option<u32>,
    pub max_age: Option<Duration>,
    pub condition: Option<GateCondition>,
    /// Optional mechanical servicing metadata. It is separate from subscribers
    /// so an automation actor does not become an LLM/process queue worker.
    pub automation: Option<QueueAutomation>,
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
    /// Verdict id -> transition id routing declared on this transition. Empty
    /// unless declared.
    pub outcomes: BTreeMap<VerdictId, TransitionId>,
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
            outcomes: transition.outcomes.clone(),
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
            automation: queue.automation.clone(),
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
            outcomes: transition.outcomes.clone(),
        })
        .collect();

    let external_tools: Vec<ExternalToolManifest> = role
        .external_tools
        .iter()
        .map(|tool| ExternalToolManifest {
            id: tool.id.clone(),
            description: tool.description.clone(),
            required: tool.required,
            constraints: tool.constraints.clone(),
            guidance: tool.guidance.clone(),
        })
        .collect();
    let prompt = build_prompt(workflow.name(), role, queues, &tools, &external_tools);

    RoleManifest {
        id: role.id.clone(),
        charter: role.charter.clone(),
        prompt_extension: role.prompt.clone(),
        concurrency: role.concurrency,
        queues: role.queues.clone(),
        authority,
        tools,
        external_tools,
        prompt,
    }
}
