//! Classification diagnostics and the collecting [`ClassificationError`].
//!
//! Split from the classification root so the diagnostic taxonomy and its
//! `Display` rendering stay separate from the
//! [`Classifier`](super::Classifier) that produces them.

use crate::artifact::ArtifactTarget;
use crate::ids::{ArtifactKindId, LabelId, StateDimensionId, StateId};
use std::error::Error;
use std::fmt;

/// A single problem found while classifying a Forge artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClassificationDiagnostic {
    /// The artifact body contained a metadata block that could not be parsed.
    MalformedMetadata { reason: String },
    /// No declared artifact kind matched the target and labels, and no metadata
    /// named a kind.
    Unclassified { target: ArtifactTarget },
    /// Several artifact kinds matched the labels equally well.
    AmbiguousArtifactKind {
        target: ArtifactTarget,
        candidates: Vec<ArtifactKindId>,
    },
    /// Metadata named an artifact kind the workflow never declared.
    UnknownMetadataKind { kind: ArtifactKindId },
    /// The resolved kind maps to a different Forge target than the artifact.
    TargetMismatch {
        kind: ArtifactKindId,
        expected: ArtifactTarget,
        actual: ArtifactTarget,
    },
    /// The resolved kind requires an identifying label that is absent.
    MissingIdentifyingLabel {
        kind: ArtifactKindId,
        label: LabelId,
    },
    /// Labels for several states of one exclusive dimension are present.
    ExclusiveStateConflict {
        dimension: StateDimensionId,
        states: Vec<StateId>,
    },
    /// A state label is present on an artifact kind for which the state is not legal.
    StateNotAllowedForArtifact {
        artifact: ArtifactKindId,
        dimension: StateDimensionId,
        state: StateId,
    },
}

impl fmt::Display for ClassificationDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClassificationDiagnostic::MalformedMetadata { reason } => {
                write!(formatter, "malformed workflow metadata: {reason}")
            }
            ClassificationDiagnostic::Unclassified { target } => {
                write!(formatter, "no artifact kind matched this {target}")
            }
            ClassificationDiagnostic::AmbiguousArtifactKind { target, candidates } => {
                write!(
                    formatter,
                    "this {target} matches several artifact kinds: {}",
                    join_ids(candidates)
                )
            }
            ClassificationDiagnostic::UnknownMetadataKind { kind } => {
                write!(
                    formatter,
                    "metadata names undeclared artifact kind `{kind}`"
                )
            }
            ClassificationDiagnostic::TargetMismatch {
                kind,
                expected,
                actual,
            } => write!(
                formatter,
                "artifact kind `{kind}` maps to a {expected} but was found on a {actual}"
            ),
            ClassificationDiagnostic::MissingIdentifyingLabel { kind, label } => write!(
                formatter,
                "artifact kind `{kind}` requires missing identifying label `{label}`"
            ),
            ClassificationDiagnostic::ExclusiveStateConflict { dimension, states } => write!(
                formatter,
                "exclusive dimension `{dimension}` has conflicting states: {}",
                join_states(states)
            ),
            ClassificationDiagnostic::StateNotAllowedForArtifact {
                artifact,
                dimension,
                state,
            } => write!(
                formatter,
                "state `{state}` in dimension `{dimension}` is not legal for artifact kind `{artifact}`"
            ),
        }
    }
}

fn join_ids(ids: &[ArtifactKindId]) -> String {
    ids.iter()
        .map(|id| format!("`{id}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn join_states(ids: &[StateId]) -> String {
    ids.iter()
        .map(|id| format!("`{id}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Error returned when a Forge artifact cannot be cleanly classified.
///
/// Carries every diagnostic found so a caller (a reconciler or an operator
/// queue) can see all problems at once instead of one at a time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassificationError {
    diagnostics: Vec<ClassificationDiagnostic>,
}

impl ClassificationError {
    /// Builds an error from the collected diagnostics.
    pub(super) fn new(diagnostics: Vec<ClassificationDiagnostic>) -> Self {
        Self { diagnostics }
    }

    /// Returns the collected diagnostics.
    pub fn diagnostics(&self) -> &[ClassificationDiagnostic] {
        &self.diagnostics
    }
}

impl fmt::Display for ClassificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "artifact classification failed with {} diagnostic(s)",
            self.diagnostics.len()
        )?;
        for diagnostic in &self.diagnostics {
            write!(formatter, "\n  - {diagnostic}")?;
        }
        Ok(())
    }
}

impl Error for ClassificationError {}
