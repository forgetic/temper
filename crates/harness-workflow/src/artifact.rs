//! Artifact target mapping.
//!
//! An artifact kind is a logical work item that maps to exactly one kind of
//! Forge artifact. [`ArtifactTarget`] names which Forge artifact type a kind is
//! projected onto so the classifier can decide whether to read a Forge issue or
//! a Forge pull request.
//!
//! This stays provider-neutral: it names the abstract Forge artifact type, not
//! a backend-specific concept.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The Forge artifact type a workflow artifact kind maps to.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactTarget {
    /// The artifact kind is represented by a Forge issue.
    #[default]
    Issue,
    /// The artifact kind is represented by a Forge pull request.
    PullRequest,
}

impl fmt::Display for ArtifactTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            ArtifactTarget::Issue => "issue",
            ArtifactTarget::PullRequest => "pull request",
        };
        formatter.write_str(text)
    }
}
