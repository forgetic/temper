//! Workflow relation types shared by specs and classified artifacts.
//!
//! Relation declarations describe allowed links between artifact kinds. Forge
//! artifacts still project concrete links through metadata item numbers; the
//! classifier combines those projections with declarations to surface typed
//! relations for planning and reconciliation.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The workflow meaning of a relation between artifacts.
#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// A child artifact belongs under a parent artifact, such as design -> epic.
    Parent,
    /// A work artifact depends on another work artifact landing first.
    Dependency,
    /// A pull request is the produced implementation for a work artifact.
    ProducedPr,
}

impl fmt::Display for RelationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            RelationKind::Parent => "parent",
            RelationKind::Dependency => "dependency",
            RelationKind::ProducedPr => "produced_pr",
        };
        formatter.write_str(text)
    }
}
