//! The validated workflow model.
//!
//! [`ValidatedWorkflow`] is the normalized, internally consistent form of a
//! workflow. It can only be produced by [`crate::validate::validate`] (its
//! constructor is crate-private), so downstream compiler and runtime APIs can
//! require a `ValidatedWorkflow` and trust that duplicate ids and undeclared
//! references have already been ruled out.

use crate::artifact::ArtifactTarget;
use crate::ids::{
    ArtifactKindId, GateId, LabelId, QueueId, RoleId, StateDimensionId, StateId, TransitionId,
};

/// A workflow that has passed static validation.
///
/// Construct one through [`crate::validate::validate`] or
/// [`crate::spec::RawWorkflowSpec::validate`]. There is no public constructor,
/// so a value of this type always reflects a checked workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedWorkflow {
    name: String,
    roles: Vec<ValidatedRole>,
    labels: Vec<LabelId>,
    artifact_kinds: Vec<ValidatedArtifactKind>,
    state_dimensions: Vec<ValidatedStateDimension>,
    queues: Vec<ValidatedQueue>,
    transitions: Vec<ValidatedTransition>,
    gates: Vec<ValidatedGate>,
}

impl ValidatedWorkflow {
    /// Builds a validated workflow. Crate-private so only validation can
    /// produce a value of this type.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        name: String,
        roles: Vec<ValidatedRole>,
        labels: Vec<LabelId>,
        artifact_kinds: Vec<ValidatedArtifactKind>,
        state_dimensions: Vec<ValidatedStateDimension>,
        queues: Vec<ValidatedQueue>,
        transitions: Vec<ValidatedTransition>,
        gates: Vec<ValidatedGate>,
    ) -> Self {
        Self {
            name,
            roles,
            labels,
            artifact_kinds,
            state_dimensions,
            queues,
            transitions,
            gates,
        }
    }

    /// Returns the workflow name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the validated roles.
    pub fn roles(&self) -> &[ValidatedRole] {
        &self.roles
    }

    /// Returns the declared label ids.
    pub fn labels(&self) -> &[LabelId] {
        &self.labels
    }

    /// Returns the validated artifact kinds.
    pub fn artifact_kinds(&self) -> &[ValidatedArtifactKind] {
        &self.artifact_kinds
    }

    /// Returns the artifact kind with the given id, if declared.
    pub fn artifact_kind(&self, id: &ArtifactKindId) -> Option<&ValidatedArtifactKind> {
        self.artifact_kinds.iter().find(|kind| &kind.id == id)
    }

    /// Returns the validated state dimensions.
    pub fn state_dimensions(&self) -> &[ValidatedStateDimension] {
        &self.state_dimensions
    }

    /// Returns the validated queues.
    pub fn queues(&self) -> &[ValidatedQueue] {
        &self.queues
    }

    /// Returns the validated transitions.
    pub fn transitions(&self) -> &[ValidatedTransition] {
        &self.transitions
    }

    /// Returns the validated gates.
    pub fn gates(&self) -> &[ValidatedGate] {
        &self.gates
    }
}

/// A validated role with typed queue references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedRole {
    pub id: RoleId,
    pub charter: Option<String>,
    /// Concurrency hint: how many artifacts the role may hold at once, or
    /// `None` for no declared limit.
    pub concurrency: Option<u32>,
    pub queues: Vec<QueueId>,
}

/// A validated artifact kind with typed label references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedArtifactKind {
    pub id: ArtifactKindId,
    /// Forge artifact type this kind maps to (issue or pull request).
    pub target: ArtifactTarget,
    /// Labels that must all be present for a Forge artifact to be classified as
    /// this kind.
    pub identifying_labels: Vec<LabelId>,
}

/// A validated state within a dimension.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedState {
    pub id: StateId,
    pub label: Option<LabelId>,
}

/// A validated state dimension and its states.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedStateDimension {
    pub id: StateDimensionId,
    /// When `true`, an artifact may occupy at most one state of this dimension.
    pub exclusive: bool,
    pub states: Vec<ValidatedState>,
}

/// A validated queue with typed artifact and label references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedQueue {
    pub id: QueueId,
    pub artifact: ArtifactKindId,
    pub labels: Vec<LabelId>,
}

/// A validated transition effect with typed references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    AddLabel(LabelId),
    RemoveLabel(LabelId),
    SetAssignee(RoleId),
    RemoveAssignee(RoleId),
    CreateComment { body: String },
    CreatePullRequest { correlation_key: Option<String> },
    MergePullRequest,
}

/// A validated transition with typed references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedTransition {
    pub id: TransitionId,
    pub artifact: ArtifactKindId,
    pub roles: Vec<RoleId>,
    pub requires_gates: Vec<GateId>,
    pub effects: Vec<Effect>,
}

/// A validated gate with typed transition references and optional condition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedGate {
    pub id: GateId,
    pub satisfied_by: Vec<TransitionId>,
    pub condition: Option<GateCondition>,
}

/// A typed portable condition that can satisfy a gate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GateCondition {
    LabelPresent(LabelId),
    StateEquals {
        dimension: StateDimensionId,
        state: StateId,
    },
}
