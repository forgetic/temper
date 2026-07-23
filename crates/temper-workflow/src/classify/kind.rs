//! Deterministic artifact-kind resolution shared by live classification and recovery.
//!
//! Label evidence is resolved without consulting metadata. Optional metadata is
//! then validated as a consistency assertion over that independent result.

use super::diagnostic::{ClassificationDiagnostic, ClassificationError};
use crate::artifact::ArtifactTarget;
use crate::ids::ArtifactKindId;
use crate::validated::{ValidatedArtifactKind, ValidatedWorkflow};
use std::collections::HashSet;

/// Internal result that retains a label-derived kind while diagnostics collect.
pub(super) struct KindResolution {
    pub(super) kind: Option<ArtifactKindId>,
    pub(super) diagnostics: Vec<ClassificationDiagnostic>,
}

impl KindResolution {
    pub(super) fn into_result(self) -> Result<ArtifactKindId, ClassificationError> {
        match self.kind {
            Some(kind) if self.diagnostics.is_empty() => Ok(kind),
            _ => Err(ClassificationError::new(self.diagnostics)),
        }
    }
}

/// Resolves label evidence, then checks optional metadata without overriding it.
pub(super) fn resolve_kind_evidence(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &HashSet<&str>,
    metadata_kind: Option<&ArtifactKindId>,
) -> KindResolution {
    let mut label_diagnostics = Vec::new();
    let label_kind = resolve_kind_from_labels(workflow, target, labels, &mut label_diagnostics);
    let mut diagnostics = Vec::new();

    if let Some(named) = metadata_kind {
        match workflow.artifact_kind(named) {
            None => diagnostics.push(ClassificationDiagnostic::UnknownMetadataKind {
                kind: named.clone(),
            }),
            Some(kind) if kind.target != target => {
                diagnostics.push(ClassificationDiagnostic::TargetMismatch {
                    kind: kind.id.clone(),
                    expected: kind.target,
                    actual: target,
                });
            }
            Some(_) => match &label_kind {
                Some(label_kind) if label_kind != named => {
                    diagnostics.push(ClassificationDiagnostic::MetadataKindDisagreement {
                        metadata_kind: named.clone(),
                        label_kind: label_kind.clone(),
                    });
                }
                _ => {}
            },
        }
    }

    diagnostics.extend(label_diagnostics);
    KindResolution {
        kind: label_kind,
        diagnostics,
    }
}

fn resolve_kind_from_labels(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &HashSet<&str>,
    diagnostics: &mut Vec<ClassificationDiagnostic>,
) -> Option<ArtifactKindId> {
    let matches: Vec<&ValidatedArtifactKind> = workflow
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

    let Some(max) = matches
        .iter()
        .map(|kind| kind.identifying_labels.len())
        .max()
    else {
        if let Some(default) = default_kind(workflow, target) {
            return Some(default.id.clone());
        }
        let mut labels = labels
            .iter()
            .map(|label| (*label).to_string())
            .collect::<Vec<_>>();
        labels.sort();
        diagnostics.push(ClassificationDiagnostic::Unclassified { target, labels });
        return None;
    };

    // The longest identifying-label set is the most specific match. Sort tied
    // candidates by id so diagnostics do not depend on declaration order.
    let mut top: Vec<&ValidatedArtifactKind> = matches
        .into_iter()
        .filter(|kind| kind.identifying_labels.len() == max)
        .collect();
    top.sort_by(|left, right| left.id.cmp(&right.id));

    if top.len() == 1 {
        Some(top[0].id.clone())
    } else {
        diagnostics.push(ClassificationDiagnostic::AmbiguousArtifactKind {
            target,
            candidates: top.iter().map(|kind| kind.id.clone()).collect(),
        });
        None
    }
}

/// The validated default (empty identifying-label) kind for this target.
fn default_kind(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
) -> Option<&ValidatedArtifactKind> {
    workflow
        .artifact_kinds()
        .iter()
        .find(|kind| kind.target == target && kind.identifying_labels.is_empty())
}
