//! Classification of Forge issues and pull requests into workflow artifacts.
//!
//! Labels are the Forge projection of workflow state, and a metadata block in
//! the artifact body carries information labels cannot represent (see
//! [`crate::metadata`]). A [`Classifier`] reads both and produces a typed
//! [`ClassifiedArtifact`], or a [`ClassificationError`] describing impossible or
//! incomplete state.
//!
//! Classification never mutates Forge state. It is the read-side interpretation
//! that later phases build transition planning and reconciliation on top of.
//!
//! This root declares the read-side data types ([`ArtifactSource`],
//! [`ClassifiedArtifact`], [`ClassifiedRelation`]); the [`Classifier`] lives in
//! the [`classifier`] submodule and its diagnostics in [`diagnostic`].

mod classifier;
mod diagnostic;

use crate::artifact::{ArtifactRef, ArtifactTarget};
use crate::ids::{ArtifactKindId, StateDimensionId, StateId};
use crate::metadata::WorkflowMetadata;
use crate::relation::RelationKind;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use temper_forge::ItemNumber;

pub use classifier::Classifier;
pub use diagnostic::{ClassificationDiagnostic, ClassificationError};

/// Where a classified artifact came from in the Forge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactSource {
    /// The artifact is a Forge issue with the given item number.
    Issue { number: ItemNumber },
    /// The artifact is a Forge pull request with the given item number.
    PullRequest { number: ItemNumber },
}

/// A Forge artifact interpreted under a validated workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassifiedArtifact {
    /// Resolved workflow artifact kind.
    pub kind: ArtifactKindId,
    /// Forge artifact type the kind maps to.
    pub target: ArtifactTarget,
    /// Where the artifact came from.
    pub source: ArtifactSource,
    /// Last Forge update timestamp, when available from the classified source.
    pub updated_at: Option<DateTime<Utc>>,
    /// Active states per dimension, derived from the artifact's labels. An
    /// exclusive dimension has at most one entry; a non-exclusive dimension may
    /// list several.
    pub states: BTreeMap<StateDimensionId, Vec<StateId>>,
    /// Parsed workflow metadata, defaulted when the body has no block.
    pub metadata: WorkflowMetadata,
    /// Typed relations read from native links or metadata through declarations.
    pub relations: Vec<ClassifiedRelation>,
    /// Raw Forge labels present on the artifact.
    pub labels: Vec<String>,
}

/// A classified relation from one artifact to another repository item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassifiedRelation {
    /// Relation meaning declared by the workflow.
    pub kind: RelationKind,
    /// Current artifact kind carrying the relation source.
    pub source: ArtifactKindId,
    /// The linked Forge item. A reference without `repository_id` means the
    /// target is in the source artifact's repository.
    pub target: ArtifactRef,
    /// Declared artifact kinds this target item may have for this relation.
    pub target_kinds: Vec<ArtifactKindId>,
}
