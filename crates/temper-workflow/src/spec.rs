//! Raw, serde-loadable workflow specification types.
//!
//! These structs mirror the shape of an authored workflow document (YAML, JSON,
//! TOML, or generated input). They use plain `String` ids and perform no
//! validation themselves. Call [`RawWorkflowSpec::validate`] to obtain a
//! [`crate::validated::ValidatedWorkflow`].
//!
//! Keeping the raw spec separate from the validated model means downstream
//! compiler and runtime APIs can require an already-validated workflow rather
//! than re-checking an arbitrary document.

use crate::artifact::ArtifactTarget;
use crate::relation::RelationKind;
use crate::validate::validate;
use crate::validated::ValidatedWorkflow;
use crate::ValidationErrors;
use serde::{Deserialize, Deserializer, Serialize};
use temper_forge::ReviewDecision;

/// Raw workflow specification as loaded from an authored document.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawWorkflowSpec {
    /// Human-facing workflow name.
    pub name: String,
    #[serde(default)]
    pub roles: Vec<RawRole>,
    #[serde(default)]
    pub labels: Vec<RawLabel>,
    #[serde(default)]
    pub artifact_kinds: Vec<RawArtifactKind>,
    #[serde(default)]
    pub state_dimensions: Vec<RawStateDimension>,
    #[serde(default)]
    pub queues: Vec<RawQueue>,
    #[serde(default)]
    pub transitions: Vec<RawTransition>,
    #[serde(default)]
    pub gates: Vec<RawGate>,
    #[serde(default)]
    pub relations: Vec<RawRelation>,
}

impl RawWorkflowSpec {
    /// Validates this raw spec into a [`ValidatedWorkflow`].
    ///
    /// Returns every detected problem as a [`ValidationErrors`] collection
    /// instead of failing on the first issue.
    pub fn validate(&self) -> Result<ValidatedWorkflow, ValidationErrors> {
        validate(self)
    }
}

/// Role declaration: an actor authority and its work queues.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawRole {
    pub id: String,
    /// Legacy prose charter that guides judgment-heavy behavior for the role.
    /// Kept for backwards compatibility and rendered as user guidance.
    #[serde(default)]
    pub charter: Option<String>,
    /// User-authored prompt extension for this role. Generated prompt mechanics
    /// stay separate from this prose and do not infer behavior from the role id.
    #[serde(default)]
    pub prompt: RawRolePrompt,
    /// User-declared non-workflow tools this role may use only if the runner
    /// binds matching providers at runtime.
    #[serde(default)]
    pub external_tools: Vec<RawExternalTool>,
    /// Optional concurrency hint: how many artifacts the role may hold at once.
    /// Compiled into the role manifest for runtime claim limits; `None` means
    /// no declared limit.
    #[serde(default)]
    pub concurrency: Option<u32>,
    /// Queues the role draws work from. Each entry references a queue id.
    #[serde(default)]
    pub queues: Vec<String>,
}

/// User-authored prompt prose for a role.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawRolePrompt {
    /// Behavioral guidance for how this role should make workflow decisions.
    #[serde(default)]
    pub guidance: Option<String>,
    /// Guidance for how this role should use tools declared for it.
    #[serde(default)]
    pub tool_guidance: Option<String>,
}

/// User-declared metadata for a non-workflow tool.
///
/// A declaration grants no executable authority by itself. The runner must bind
/// a matching provider for the tool before a real role worker may present it as
/// available to an LLM.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawExternalTool {
    /// Tool id local to this role, such as `coding_workspace`.
    pub id: String,
    /// Human-facing capability summary.
    pub description: String,
    /// Whether a real role worker must have a runner binding before starting.
    #[serde(default)]
    pub required: bool,
    /// User-authored constraints that narrow how the bound provider may be used.
    #[serde(default)]
    pub constraints: Vec<String>,
    /// Optional prompt guidance for when and how to use the tool.
    #[serde(default)]
    pub guidance: Option<String>,
}

/// Label declaration. Labels are the public Forge projection of workflow state.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawLabel {
    pub id: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Artifact-kind declaration: a logical item mapped to a Forge issue or PR.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawArtifactKind {
    pub id: String,
    /// Forge artifact type this kind maps to (issue or pull request).
    pub target: ArtifactTarget,
    /// Labels that identify this artifact kind. A Forge artifact is classified
    /// as this kind only when all identifying labels are present. Each entry
    /// references a label id.
    #[serde(default)]
    pub identifying_labels: Vec<String>,
}

/// State dimension declaration: a named, usually mutually exclusive, state group.
///
/// `exclusive` defaults to `true`: an artifact may carry the label for at most
/// one state of the dimension at a time. Set it to `false` for dimensions whose
/// states can coexist.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawStateDimension {
    pub id: String,
    #[serde(default = "default_true")]
    pub exclusive: bool,
    #[serde(default)]
    pub states: Vec<RawState>,
}

impl Default for RawStateDimension {
    fn default() -> Self {
        Self {
            id: String::new(),
            exclusive: true,
            states: Vec::new(),
        }
    }
}

/// Serde default for [`RawStateDimension::exclusive`].
fn default_true() -> bool {
    true
}

/// A single state within a state dimension.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawState {
    pub id: String,
    /// Optional label that projects this state onto a Forge label.
    #[serde(default)]
    pub label: Option<String>,
    /// Artifact kinds this state is legal for. Empty means every artifact kind.
    #[serde(default)]
    pub artifacts: Vec<String>,
}

/// Queue declaration: a query over artifacts that need attention.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawQueue {
    pub id: String,
    /// Artifact kind or kinds the queue selects. References artifact-kind ids.
    /// Serde accepts the legacy single string form or a list at the `artifact`
    /// field so existing specs remain valid.
    #[serde(rename = "artifact", deserialize_with = "deserialize_one_or_many")]
    pub artifacts: Vec<String>,
    /// Labels that must be present for an artifact to match every branch. Each
    /// entry references a label id.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Alternative label sets. When present, at least one set must match in
    /// addition to the common `labels` list.
    #[serde(default)]
    pub any_of: Vec<RawQueueLabelSet>,
    /// Optional depth threshold before the queue should be serviced.
    #[serde(default)]
    pub min_depth: Option<u32>,
    /// Optional age threshold in seconds for the oldest matched member.
    #[serde(default)]
    pub max_age: Option<u32>,
    /// Optional native/projected condition that must hold for the queue to match.
    #[serde(default)]
    pub condition: Option<RawGateCondition>,
}

/// One AND-clause in a queue's disjunctive label filter.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawQueueLabelSet {
    /// Labels that must all be present for this alternative to match.
    #[serde(default)]
    pub labels: Vec<String>,
}

fn deserialize_one_or_many<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }

    match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(value) => Ok(vec![value]),
        OneOrMany::Many(values) => Ok(values),
    }
}

/// Transition declaration: a guarded, role-authorized state change.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawTransition {
    pub id: String,
    /// Artifact kind the transition operates on. References an artifact-kind id.
    pub artifact: String,
    /// Roles authorized to perform the transition. Each entry references a role id.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Gates that must be satisfied before the transition can run. Each entry
    /// references a gate id.
    #[serde(default)]
    pub requires_gates: Vec<String>,
    /// Effects applied when the transition runs.
    #[serde(default)]
    pub effects: Vec<RawEffect>,
}

/// A raw transition effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawEffect {
    /// Add a label to the target artifact. References a label id.
    AddLabel { label: String },
    /// Remove a label from the target artifact. References a label id.
    RemoveLabel { label: String },
    /// Assign the target artifact to the worker/user resolved for `role`.
    ///
    /// The payload references a declared workflow role, not a concrete Forge
    /// user. Runtime resolution of role-to-user/worker is deferred to the
    /// executor/runner layer.
    SetAssignee { role: String },
    /// Remove the assignee resolved for `role` from the target artifact.
    ///
    /// As with [`RawEffect::SetAssignee`], `role` is a declared workflow role
    /// id rather than a concrete Forge user id.
    RemoveAssignee { role: String },
    /// Post a prose/template comment body on the target artifact.
    CreateComment { body: String },
    /// Request creation of a pull request.
    ///
    /// `correlation_key`, when present, identifies retries of the same create
    /// request. Branches, title, body, and labels come from runtime context at
    /// execution time.
    CreatePullRequest {
        #[serde(default)]
        correlation_key: Option<String>,
    },
    /// Request reviews from users resolved for workflow roles on the target PR.
    RequestReviewers { roles: Vec<String> },
    /// Submit a native pull-request review as the backend client's current user.
    SubmitReview { decision: ReviewDecision },
    /// Request merging the target pull request. Carries no portable payload.
    MergePullRequest,
}

/// Relation declaration: an allowed typed link between artifact kinds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawRelation {
    /// The workflow meaning of the link.
    pub kind: RelationKind,
    /// Artifact kind that carries the relation source.
    pub source: String,
    /// Artifact kind that the linked item number points at.
    pub target: String,
}

/// Gate declaration: a condition that unlocks transitions.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawGate {
    pub id: String,
    /// Transitions whose completion satisfies this gate. Each entry references
    /// a transition id.
    #[serde(default)]
    pub satisfied_by: Vec<String>,
    /// Portable condition that can satisfy this gate, such as a projected
    /// label/state condition or a runtime-supplied native CI/dependency signal.
    #[serde(default)]
    pub condition: Option<RawGateCondition>,
}

/// A portable condition that can satisfy a gate without a workflow transition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawGateCondition {
    /// The artifact must carry this label.
    LabelPresent { label: String },
    /// The artifact must occupy `state` within `dimension`.
    StateEquals { dimension: String, state: String },
    /// Every `dependency` relation target of the artifact must have landed
    /// (its prerequisite work merged). Which targets have landed is supplied by
    /// the runtime as an external signal; the condition references relations by
    /// kind, so it carries no payload.
    DependenciesResolved,
    /// The artifact's native CI must have passed. Whether CI passed is supplied
    /// by the runtime as a signal computed from the Forge's `CiJob`
    /// conclusions (see ADR 0014); the condition references the artifact's CI,
    /// so it carries no payload.
    CiPassed,
    /// The artifact's native CI must have completed with a non-success result.
    /// The runtime computes this from the same Forge `CiJob` data as
    /// [`RawGateCondition::CiPassed`].
    CiFailed,
    /// The pull request's native review aggregate must be approved.
    ReviewApproved,
    /// Some reviewer's latest native review decision must request changes.
    ReviewChangesRequested,
}
