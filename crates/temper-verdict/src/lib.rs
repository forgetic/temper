// SPDX-License-Identifier: MPL-2.0

//! Shared machine-readable verdict output contracts and pure validation.
//!
//! This leaf crate is used at the agent, worker, and engine trust boundaries so
//! every tier enforces the same payload shape without trusting an upstream tier.

mod validation;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use validation::{VerdictValidationError, validate_verdict_result};

/// Requirements for one workflow-declared verdict's terminal result.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct VerdictContract {
    #[serde(default)]
    pub min_children: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_children: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_child_kinds: Vec<String>,
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
