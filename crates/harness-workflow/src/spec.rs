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
use crate::validate::validate;
use crate::validated::ValidatedWorkflow;
use crate::ValidationErrors;
use serde::{Deserialize, Serialize};

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
    /// Prose charter that guides judgment-heavy behavior for the role.
    #[serde(default)]
    pub charter: Option<String>,
    /// Optional concurrency hint: how many artifacts the role may hold at once.
    /// Compiled into the role manifest for runtime claim limits; `None` means
    /// no declared limit.
    #[serde(default)]
    pub concurrency: Option<u32>,
    /// Queues the role draws work from. Each entry references a queue id.
    #[serde(default)]
    pub queues: Vec<String>,
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
}

/// Queue declaration: a query over artifacts that need attention.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawQueue {
    pub id: String,
    /// Artifact kind the queue selects. References an artifact-kind id.
    pub artifact: String,
    /// Labels that must be present for an artifact to match. Each entry
    /// references a label id.
    #[serde(default)]
    pub labels: Vec<String>,
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

/// A raw transition effect. Phase 2 models only label projection effects.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawEffect {
    /// Add a label to the artifact. References a label id.
    AddLabel { label: String },
    /// Remove a label from the artifact. References a label id.
    RemoveLabel { label: String },
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
}
