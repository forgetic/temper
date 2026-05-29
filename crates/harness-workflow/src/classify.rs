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

use crate::artifact::ArtifactTarget;
use crate::ids::{ArtifactKindId, LabelId, StateDimensionId, StateId};
use crate::metadata::{parse_metadata_block, WorkflowMetadata};
use crate::validated::{ValidatedArtifactKind, ValidatedWorkflow};
use harness_forge::{Issue, ItemNumber, PullRequest};
use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;

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
    /// Active states per dimension, derived from the artifact's labels. An
    /// exclusive dimension has at most one entry; a non-exclusive dimension may
    /// list several.
    pub states: BTreeMap<StateDimensionId, Vec<StateId>>,
    /// Parsed workflow metadata, defaulted when the body has no block.
    pub metadata: WorkflowMetadata,
    /// Raw Forge labels present on the artifact.
    pub labels: Vec<String>,
}

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

/// Classifies Forge artifacts against a validated workflow.
pub struct Classifier<'a> {
    workflow: &'a ValidatedWorkflow,
}

impl<'a> Classifier<'a> {
    /// Creates a classifier bound to a validated workflow.
    pub fn new(workflow: &'a ValidatedWorkflow) -> Self {
        Self { workflow }
    }

    /// Classifies a Forge issue.
    pub fn classify_issue(&self, issue: &Issue) -> Result<ClassifiedArtifact, ClassificationError> {
        self.classify(
            ArtifactTarget::Issue,
            ArtifactSource::Issue {
                number: issue.number,
            },
            &issue.labels,
            &issue.body,
        )
    }

    /// Classifies a Forge pull request.
    pub fn classify_pull_request(
        &self,
        pull_request: &PullRequest,
    ) -> Result<ClassifiedArtifact, ClassificationError> {
        self.classify(
            ArtifactTarget::PullRequest,
            ArtifactSource::PullRequest {
                number: pull_request.number,
            },
            &pull_request.labels,
            &pull_request.body,
        )
    }

    fn classify(
        &self,
        target: ArtifactTarget,
        source: ArtifactSource,
        labels: &[String],
        body: &str,
    ) -> Result<ClassifiedArtifact, ClassificationError> {
        let mut diagnostics = Vec::new();

        let metadata = match parse_metadata_block(body) {
            Ok(Some(metadata)) => metadata,
            Ok(None) => WorkflowMetadata::default(),
            Err(error) => {
                diagnostics.push(ClassificationDiagnostic::MalformedMetadata {
                    reason: error.to_string(),
                });
                WorkflowMetadata::default()
            }
        };

        let label_set: HashSet<&str> = labels.iter().map(String::as_str).collect();
        let kind = self.resolve_kind(target, &label_set, &metadata, &mut diagnostics);
        let states = self.resolve_states(&label_set, &mut diagnostics);

        match kind {
            Some(kind) if diagnostics.is_empty() => Ok(ClassifiedArtifact {
                kind,
                target,
                source,
                states,
                metadata,
                labels: labels.to_vec(),
            }),
            _ => Err(ClassificationError { diagnostics }),
        }
    }

    /// Resolves the artifact kind. Metadata `kind` is authoritative when set;
    /// otherwise the kind is inferred from identifying labels.
    fn resolve_kind(
        &self,
        target: ArtifactTarget,
        labels: &HashSet<&str>,
        metadata: &WorkflowMetadata,
        diagnostics: &mut Vec<ClassificationDiagnostic>,
    ) -> Option<ArtifactKindId> {
        if let Some(named) = &metadata.kind {
            return self.resolve_named_kind(target, labels, named, diagnostics);
        }
        self.resolve_kind_from_labels(target, labels, diagnostics)
    }

    fn resolve_named_kind(
        &self,
        target: ArtifactTarget,
        labels: &HashSet<&str>,
        named: &ArtifactKindId,
        diagnostics: &mut Vec<ClassificationDiagnostic>,
    ) -> Option<ArtifactKindId> {
        let Some(kind) = self.workflow.artifact_kind(named) else {
            diagnostics.push(ClassificationDiagnostic::UnknownMetadataKind {
                kind: named.clone(),
            });
            return None;
        };
        if kind.target != target {
            diagnostics.push(ClassificationDiagnostic::TargetMismatch {
                kind: kind.id.clone(),
                expected: kind.target,
                actual: target,
            });
        }
        for label in &kind.identifying_labels {
            if !labels.contains(label.as_str()) {
                diagnostics.push(ClassificationDiagnostic::MissingIdentifyingLabel {
                    kind: kind.id.clone(),
                    label: label.clone(),
                });
            }
        }
        Some(kind.id.clone())
    }

    fn resolve_kind_from_labels(
        &self,
        target: ArtifactTarget,
        labels: &HashSet<&str>,
        diagnostics: &mut Vec<ClassificationDiagnostic>,
    ) -> Option<ArtifactKindId> {
        let matches: Vec<&ValidatedArtifactKind> = self
            .workflow
            .artifact_kinds()
            .iter()
            .filter(|kind| kind.target == target)
            .filter(|kind| !kind.identifying_labels.is_empty())
            .filter(|kind| {
                kind.identifying_labels
                    .iter()
                    .all(|label| labels.contains(label.as_str()))
            })
            .collect();

        let Some(max) = matches.iter().map(|k| k.identifying_labels.len()).max() else {
            diagnostics.push(ClassificationDiagnostic::Unclassified { target });
            return None;
        };

        // Prefer the most specific match: the kind requiring the most
        // identifying labels wins. A tie at the top is genuinely ambiguous.
        let top: Vec<&ValidatedArtifactKind> = matches
            .into_iter()
            .filter(|k| k.identifying_labels.len() == max)
            .collect();

        if top.len() == 1 {
            Some(top[0].id.clone())
        } else {
            diagnostics.push(ClassificationDiagnostic::AmbiguousArtifactKind {
                target,
                candidates: top.iter().map(|k| k.id.clone()).collect(),
            });
            None
        }
    }

    fn resolve_states(
        &self,
        labels: &HashSet<&str>,
        diagnostics: &mut Vec<ClassificationDiagnostic>,
    ) -> BTreeMap<StateDimensionId, Vec<StateId>> {
        let mut states = BTreeMap::new();
        for dimension in self.workflow.state_dimensions() {
            let active: Vec<StateId> = dimension
                .states
                .iter()
                .filter(|state| {
                    state
                        .label
                        .as_ref()
                        .is_some_and(|label| labels.contains(label.as_str()))
                })
                .map(|state| state.id.clone())
                .collect();

            if active.is_empty() {
                continue;
            }
            if dimension.exclusive && active.len() > 1 {
                diagnostics.push(ClassificationDiagnostic::ExclusiveStateConflict {
                    dimension: dimension.id.clone(),
                    states: active.clone(),
                });
            }
            states.insert(dimension.id.clone(), active);
        }
        states
    }
}
