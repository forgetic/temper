// SPDX-License-Identifier: MPL-2.0

//! Shared machine-readable verdict output contracts and pure validation.
//!
//! This leaf crate is used at the agent, worker, and engine trust boundaries so
//! every tier enforces the same payload shape without trusting an upstream tier.

mod validation;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use validation::{VerdictValidationError, validate_verdict_result};

/// An engine-resolved target branch that every child product must use.
///
/// This is deliberately a resolved wire contract rather than a workflow policy:
/// agents and workers do not need workflow vocabulary in order to enforce the
/// exact value selected from fresh engine state. Older contexts omit the field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TargetBranchRequirement {
    /// Exact branch value accepted in explicitly authored child metadata.
    pub expected: String,
    /// Repository default observed while resolving the policy. Keeping it in the
    /// contract lets every validation tier identify an accidental default branch
    /// rather than reporting only a generic mismatch.
    pub repository_default: String,
    /// Whether the child may omit `target_branch` so the engine can stamp
    /// [`Self::expected`] at the mutation boundary.
    #[serde(default)]
    pub allow_omission: bool,
}

/// A required subset of child products with optional dependency coverage.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChildKindRequirement {
    pub kind: String,
    #[serde(default)]
    pub min_children: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_children: Option<usize>,
    /// Every child of [`Self::kind`] must depend on every sibling whose kind is
    /// listed here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on_all_kinds: Vec<String>,
}

/// Requirements for one workflow-declared verdict's terminal result.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct VerdictContract {
    #[serde(default)]
    pub min_children: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_children: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_child_kinds: Vec<String>,
    /// Non-blank workflow metadata keys required in every authored child body.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_child_metadata: Vec<String>,
    /// Per-kind cardinality/dependency requirements within the total child set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_kind_requirements: Vec<ChildKindRequirement>,
    /// Exact child branch resolved by the engine from a typed workflow policy.
    /// Absence preserves the legacy metadata contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_branch: Option<TargetBranchRequirement>,
    #[serde(default)]
    pub requires_pr_title: bool,
    #[serde(default)]
    pub requires_pr_body: bool,
    #[serde(default)]
    pub requires_body: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_source_metadata: Vec<String>,
}

pub type VerdictContracts = BTreeMap<String, VerdictContract>;
pub type SourceMetadata = BTreeMap<String, String>;

/// Read-only view of a terminal result accepted by the shared validator.
pub trait VerdictResultView {
    type Child: VerdictChildView;

    fn verdict(&self) -> Option<&str>;
    fn title(&self) -> Option<&str>;
    fn body(&self) -> Option<&str>;
    fn children(&self) -> &[Self::Child];
}

/// Read-only view of one authored child product.
pub trait VerdictChildView {
    fn slug(&self) -> &str;
    fn title(&self) -> &str;
    fn body(&self) -> &str;
    fn kind(&self) -> Option<&str>;
    fn depends_on(&self) -> &[String];
}
