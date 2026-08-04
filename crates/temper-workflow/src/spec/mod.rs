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

mod effects;

pub use effects::{RawChildKindRequirement, RawEffect, RawGateCondition, TargetBranchPolicy};

use crate::ValidationErrors;
use crate::artifact::ArtifactTarget;
use crate::relation::RelationKind;
use crate::validate::validate;
use crate::validated::ValidatedWorkflow;
use serde::{Deserialize, Deserializer, Serialize};

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
    /// Workflow-declared validator handoffs. These are parsed and validated as
    /// stable policy data, but not evaluated or enqueued by this crate yet.
    #[serde(default)]
    pub validation_bindings: Vec<RawValidationBinding>,
    /// Who is expected to file intake (the "external filer"). When seeding an
    /// intake issue, provisioning authors it as this identity. `None` keeps the
    /// legacy behavior of authoring as the `human` role.
    #[serde(default)]
    pub intake_author: Option<RawIntakeAuthor>,
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

/// Declares a workflow-owned validation handoff policy.
///
/// A binding names the validator role/action to run, the artifact kind being
/// judged, opaque trigger/readiness/selection/aggregation policy data for later
/// runtime work, and the idempotency key template that future enqueuing will use
/// to avoid duplicate validation jobs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawValidationBinding {
    /// Stable workflow-local validation binding id.
    pub id: String,
    /// Role id assigned to perform validation.
    pub role: String,
    /// Workflow action/transition id assigned to the validator role.
    pub action: String,
    /// Artifact kind selected as the validation target.
    pub target_artifact: String,
    /// Opaque trigger criteria, preserved for future runtime evaluation.
    pub trigger: RawValidationBindingDetail,
    /// Opaque readiness criteria, preserved for future runtime evaluation.
    pub readiness: RawValidationBindingDetail,
    /// Opaque target-selection policy, preserved for future runtime evaluation.
    pub target_selection: RawValidationBindingDetail,
    /// Opaque aggregation policy, preserved for future runtime evaluation.
    pub aggregation: RawValidationBindingDetail,
    /// Template for deduplicating validation work for a target state.
    pub idempotency_key: String,
}

/// Forward-compatible validation-binding policy detail.
///
/// Authoring can use prose while semantics are still being designed, or a JSON
/// value that later runtime scanner/planner code can interpret without changing
/// the top-level binding field names.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RawValidationBindingDetail {
    /// Human-authored prose description of the policy detail.
    Description(String),
    /// Structured placeholder retained exactly as supplied by the workflow.
    Structured(serde_json::Value),
}

/// Declares who is expected to file intake into the workflow.
///
/// JSON forms: `{ "kind": "role", "role": "human" }` for a provisioned workflow
/// role, or `{ "kind": "site_admin" }` for the provisioning admin identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawIntakeAuthor {
    /// A provisioned workflow role (e.g. `human`) that authors the intake issue.
    Role { role: String },
    /// The provisioning admin identity (the "external filer") authors the intake.
    SiteAdmin,
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
    /// Labels attached when the engine creates an artifact of this kind, in
    /// addition to the identifying labels. Unlike identifying labels they are
    /// not part of the kind's identity: later transitions may freely remove
    /// them (e.g. an initial `needs-reviewer` routing label cleared by the
    /// review). Each entry references a label id.
    #[serde(default)]
    pub initial_labels: Vec<String>,
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
    /// Labels that must be absent for an artifact to match. Each entry
    /// references a label id. This lets a queue keep durable handoff labels
    /// intact while a temporary blocker label routes the artifact elsewhere.
    #[serde(default)]
    pub excluded_labels: Vec<String>,
    /// Alternative label sets. When present, at least one set must match in
    /// addition to the common `labels` list.
    #[serde(default)]
    pub any_of: Vec<RawQueueLabelSet>,
    /// Whether periodic discovery may evaluate this queue on terminal
    /// (closed or merged) artifacts. Terminal queues must have positive label
    /// evidence or select only artifact kinds with identifying labels.
    #[serde(default)]
    pub terminal: bool,
    /// Optional depth threshold before the queue should be serviced.
    #[serde(default)]
    pub min_depth: Option<u32>,
    /// Optional age threshold in seconds for the oldest matched member.
    #[serde(default)]
    pub max_age: Option<u32>,
    /// Optional native/projected condition that must hold for the queue to match.
    #[serde(default)]
    pub condition: Option<RawGateCondition>,
    /// Optional mechanical servicing declaration. This does not change queue
    /// matching; it names the role authority and transition a runner may use to
    /// service already-matched active queue members.
    #[serde(default)]
    pub automation: Option<RawQueueAutomation>,
    /// Role-worker action assignments for matched active queue members. Each
    /// entry binds a subscribed role (and optionally one artifact kind in a
    /// multi-kind queue) to the workflow transition/action the worker should run.
    #[serde(default)]
    pub actions: Vec<RawQueueAction>,
}

/// Role-worker action assignment metadata for a queue.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawQueueAction {
    /// Role id whose worker should receive this assignment.
    pub role: String,
    /// Optional artifact-kind discriminator for multi-kind queues.
    #[serde(default)]
    pub artifact: Option<String>,
    /// Workflow transition/action id assigned to the role worker.
    pub action: String,
    /// Optional checkout capability override for this action assignment:
    /// `writable`, `read_only`, `pull_request_read_only`, or
    /// `pull_request_writable`.
    #[serde(default)]
    pub checkout: Option<String>,
    /// Optional job-specific guidance appended to generated guidance.
    #[serde(default)]
    pub guidance: Option<String>,
}

/// Mechanical servicing metadata for a queue.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawQueueAutomation {
    /// Workflow role id whose authority is used to execute the transition.
    pub actor: String,
    /// Transition to run for matched active queue members.
    pub transition: String,
    /// Optional external-tool id of a workspace executor that services this
    /// automation directly, instead of running `transition` mechanically. When
    /// set, the mechanical worker invokes the workspace bound for `actor` under
    /// this id, then routes on the workspace's returned verdict through
    /// `outcomes` (ADR 0022 §D). The id must be declared on the `actor` role's
    /// `external_tools`. When unset the automation runs `transition` directly,
    /// as before.
    #[serde(default)]
    pub executor: Option<String>,
    /// Optional fallback transition to run when the primary transition fails
    /// because the PR cannot be merged cleanly. This is sugar over `outcomes`:
    /// it desugars into an outcome keyed by the built-in merge-conflict verdict.
    #[serde(default)]
    pub on_merge_conflict: Option<String>,
    /// General verdict id -> transition id routing for this automation.
    ///
    /// Verdict ids are opaque workflow vocabulary; the engine validates only
    /// that each maps to a transition legal for the actor on the queue's
    /// artifact. `on_merge_conflict` desugars into an entry keyed by the
    /// built-in merge-conflict verdict.
    #[serde(default)]
    pub outcomes: std::collections::BTreeMap<String, String>,
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
    /// Verdict id -> transition id routing for a workspace-backed action that
    /// exposes this transition as a tool. When the action's workspace returns a
    /// verdict, the engine runs the mapped transition instead of this one.
    ///
    /// Verdict ids are opaque workflow vocabulary; the engine validates only
    /// that each maps to a transition legal for this action's artifact/role.
    #[serde(default)]
    pub outcomes: std::collections::BTreeMap<String, String>,
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
