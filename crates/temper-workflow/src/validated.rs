//! The validated workflow model.
//!
//! [`ValidatedWorkflow`] is the normalized, internally consistent form of a
//! workflow. It can only be produced by [`crate::validate::validate`] (its
//! constructor is crate-private), so downstream compiler and runtime APIs can
//! require a `ValidatedWorkflow` and trust that duplicate ids and undeclared
//! references have already been ruled out.

use crate::artifact::ArtifactTarget;
use crate::ids::{
    ArtifactKindId, ExternalToolId, GateId, LabelId, QueueId, RoleId, StateDimensionId, StateId,
    TransitionId, VerdictId,
};
use crate::relation::RelationKind;
use chrono::Duration;
use std::collections::BTreeMap;
use temper_forge::ReviewDecision;

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
    relations: Vec<ValidatedRelation>,
    intake_author: Option<IntakeAuthor>,
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
        relations: Vec<ValidatedRelation>,
        intake_author: Option<IntakeAuthor>,
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
            relations,
            intake_author,
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

    /// Returns the validated relation declarations.
    pub fn relations(&self) -> &[ValidatedRelation] {
        &self.relations
    }

    /// Returns who is expected to file intake, if the workflow declares it.
    ///
    /// `None` means the workflow did not set the knob; provisioning then keeps
    /// the legacy behavior of authoring intake as the `human` role.
    pub fn intake_author(&self) -> Option<&IntakeAuthor> {
        self.intake_author.as_ref()
    }
}

/// Who is expected to file intake into the workflow (the "external filer").
///
/// The validated form references a declared role by typed [`RoleId`]; validation
/// guarantees the `Role` variant names a role the workflow declares.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntakeAuthor {
    /// A provisioned workflow role authors the intake issue.
    Role(RoleId),
    /// The provisioning admin identity authors the intake issue.
    SiteAdmin,
}

/// A validated role with typed queue references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedRole {
    pub id: RoleId,
    /// Legacy user guidance carried from `RawRole::charter`.
    pub charter: Option<String>,
    /// Structured user-authored prompt extension for this role.
    pub prompt: RolePromptExtension,
    /// User-declared non-workflow tools, in declaration order.
    pub external_tools: Vec<ExternalToolDeclaration>,
    /// Concurrency hint: how many artifacts the role may hold at once, or
    /// `None` for no declared limit.
    pub concurrency: Option<u32>,
    pub queues: Vec<QueueId>,
}

/// User-authored prompt prose attached to a role.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct RolePromptExtension {
    /// Behavioral guidance for role judgment. It is rendered separately from
    /// generated workflow mechanics and grants no authority by itself.
    pub guidance: Option<String>,
    /// User guidance for using declared tools. It is advisory only; executable
    /// authority still comes from the compiled tool manifest.
    pub tool_guidance: Option<String>,
}

/// A validated external tool declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalToolDeclaration {
    /// Tool id local to this role.
    pub id: ExternalToolId,
    /// Human-facing capability summary.
    pub description: String,
    /// Whether a matching runner binding is required before a real worker starts.
    pub required: bool,
    /// User-authored constraints on the external provider's use.
    pub constraints: Vec<String>,
    /// Optional prompt guidance for this tool.
    pub guidance: Option<String>,
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
    /// Labels attached when the engine creates an artifact of this kind, in
    /// addition to the identifying labels. They are not part of the kind's
    /// identity and later transitions may freely remove them.
    pub initial_labels: Vec<LabelId>,
}

/// A validated state within a dimension.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedState {
    pub id: StateId,
    pub label: Option<LabelId>,
    /// Artifact kinds this state is legal for. Empty means every artifact kind.
    pub artifacts: Vec<ArtifactKindId>,
}

impl ValidatedState {
    /// Returns whether this state is legal for the given artifact kind.
    pub fn allows_artifact(&self, artifact: &ArtifactKindId) -> bool {
        self.artifacts.is_empty() || self.artifacts.contains(artifact)
    }
}

/// A validated state dimension and its states.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedStateDimension {
    pub id: StateDimensionId,
    /// When `true`, an artifact may occupy at most one state of this dimension.
    pub exclusive: bool,
    pub states: Vec<ValidatedState>,
}

/// A validated queue with typed artifact, label, activation, and automation references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedQueue {
    pub id: QueueId,
    pub artifacts: Vec<ArtifactKindId>,
    pub labels: Vec<LabelId>,
    pub any_of: Vec<QueueLabelSet>,
    pub min_depth: Option<u32>,
    pub max_age: Option<Duration>,
    pub condition: Option<GateCondition>,
    /// Optional mechanical servicing declaration. Matching and activation stay
    /// pure; this only describes which actor/transition may service members.
    pub automation: Option<QueueAutomation>,
    /// Role-worker action assignments for matched active queue members.
    pub actions: Vec<QueueAction>,
}

/// Role-worker action assignment metadata for a validated or compiled queue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueAction {
    pub role: RoleId,
    pub artifact: Option<ArtifactKindId>,
    pub action: TransitionId,
    pub checkout: Option<String>,
    pub guidance: Option<String>,
}

/// Mechanical servicing metadata for a validated or compiled queue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueAutomation {
    pub actor: RoleId,
    pub transition: TransitionId,
    /// Optional workspace executor (declared external-tool id) that services
    /// this automation directly. When set, the mechanical worker invokes the
    /// workspace bound for `actor` under this id and routes on its returned
    /// verdict through `outcomes`, instead of running `transition` mechanically
    /// (ADR 0022 §D). When `None` the automation runs `transition` directly.
    pub executor: Option<ExternalToolId>,
    /// Verdict id -> transition id routing. The merge-conflict fallback (the
    /// `on_merge_conflict` sugar) lives here under the built-in merge-conflict
    /// verdict; see [`VerdictId::merge_conflict`].
    pub outcomes: BTreeMap<VerdictId, TransitionId>,
}

impl QueueAutomation {
    /// Returns the transition the built-in merge-conflict verdict routes to, if
    /// the automation declares one (the first instance of verdict routing).
    pub fn merge_conflict(&self) -> Option<&TransitionId> {
        self.outcomes.get(&VerdictId::merge_conflict())
    }

    /// Returns the transition a verdict routes to, if declared.
    pub fn outcome(&self, verdict: &VerdictId) -> Option<&TransitionId> {
        self.outcomes.get(verdict)
    }
}

/// One AND-clause in a queue's disjunctive label filter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueLabelSet {
    pub labels: Vec<LabelId>,
}

/// A validated transition effect with typed references.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    AddLabel(LabelId),
    RemoveLabel(LabelId),
    RemoveLabelIfPresent(LabelId),
    SetAssignee(RoleId),
    RemoveAssignee(RoleId),
    CreateComment {
        body: String,
    },
    CreatePullRequest {
        correlation_key: Option<String>,
    },
    RequestReviewers {
        roles: Vec<RoleId>,
    },
    SubmitReview {
        decision: ReviewDecision,
    },
    /// Write an agent-authored body onto the current artifact. The body text
    /// comes from the workspace work product through a keyed runtime-input seam,
    /// exactly as `CreatePullRequest` reads its head; the optional correlation
    /// key identifies retries of the same authored write.
    SetBody {
        correlation_key: Option<String>,
    },
    /// Submit a native pull-request review carrying the work product's review
    /// body. The decision is declared workflow vocabulary (which transition
    /// submits which verdict); the body text is supplied at runtime through the
    /// same keyed seam. The optional correlation key identifies retries.
    AttachReview {
        decision: ReviewDecision,
        correlation_key: Option<String>,
    },
    /// Create one-or-many child artifacts with workspace-authored
    /// titles/bodies/labels and declared parent/dependency relations. The
    /// children come from the workspace work product through a keyed
    /// runtime-input seam, exactly as `CreatePullRequest` reads its head; the
    /// optional correlation key is the base under which each child is made
    /// idempotent across retries.
    CreateIssues {
        correlation_key: Option<String>,
    },
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
    /// Verdict id -> transition id routing for a workspace-backed action that
    /// exposes this transition as a tool. Empty unless declared.
    pub outcomes: BTreeMap<VerdictId, TransitionId>,
}

/// A validated relation declaration with typed artifact-kind endpoints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedRelation {
    pub kind: RelationKind,
    pub source: ArtifactKindId,
    pub target: ArtifactKindId,
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
    /// Satisfied when every `dependency` relation target of the artifact has
    /// landed, per runtime-supplied dependency status. Vacuously true for an
    /// artifact with no dependency relations.
    DependenciesResolved,
    /// Satisfied when the artifact's native CI passed, per the runtime-supplied
    /// CI signal computed from the Forge's `CiJob` conclusions (see ADR 0014).
    CiPassed,
    /// Satisfied when native CI completed with a non-success conclusion.
    CiFailed,
    /// Satisfied when the target pull request's native review aggregate is approved.
    ReviewApproved,
    /// Satisfied when a latest native review decision requests changes.
    ReviewChangesRequested,
}
