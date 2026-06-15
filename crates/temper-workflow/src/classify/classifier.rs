//! The [`Classifier`] that interprets Forge artifacts under a validated workflow.
//!
//! Reads labels (the Forge projection of workflow state) and the artifact body's
//! metadata block, and produces a typed [`ClassifiedArtifact`] or a
//! [`ClassificationError`]. Classification never mutates Forge state. Split from
//! the classification root to keep each file within the source-size budget.

use super::diagnostic::{ClassificationDiagnostic, ClassificationError};
use super::{ArtifactSource, ClassifiedArtifact, ClassifiedRelation};
use crate::artifact::{ArtifactRef, ArtifactTarget};
use crate::ids::{ArtifactKindId, StateDimensionId, StateId};
use crate::metadata::{WorkflowMetadata, parse_metadata_block};
use crate::relation::RelationKind;
use crate::validated::{ValidatedArtifactKind, ValidatedWorkflow};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, HashSet};
use temper_forge::{Issue, ItemNumber, PullRequest};

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
            &issue.dependencies,
            Some(issue.updated_at),
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
            &pull_request.dependencies,
            Some(pull_request.updated_at),
        )
    }

    /// Classifies an artifact from its source, labels, and body.
    ///
    /// The Forge target is inferred from `source`. This is the entry point a
    /// reconciler uses when it already holds a snapshot of an artifact rather
    /// than a full [`Issue`] or [`PullRequest`], and it returns the same
    /// [`ClassificationError`] for impossible or incomplete state so the
    /// reconciler can act on it.
    pub fn classify_snapshot(
        &self,
        source: ArtifactSource,
        labels: &[String],
        body: &str,
    ) -> Result<ClassifiedArtifact, ClassificationError> {
        let target = match source {
            ArtifactSource::Issue { .. } => ArtifactTarget::Issue,
            ArtifactSource::PullRequest { .. } => ArtifactTarget::PullRequest,
        };
        self.classify(target, source, labels, body, &[], None)
    }

    /// Classifies an artifact snapshot with native dependency links.
    pub fn classify_snapshot_with_dependencies(
        &self,
        source: ArtifactSource,
        labels: &[String],
        body: &str,
        dependencies: &[ItemNumber],
    ) -> Result<ClassifiedArtifact, ClassificationError> {
        let target = match source {
            ArtifactSource::Issue { .. } => ArtifactTarget::Issue,
            ArtifactSource::PullRequest { .. } => ArtifactTarget::PullRequest,
        };
        self.classify(target, source, labels, body, dependencies, None)
    }

    fn classify(
        &self,
        target: ArtifactTarget,
        source: ArtifactSource,
        labels: &[String],
        body: &str,
        dependencies: &[ItemNumber],
        updated_at: Option<DateTime<Utc>>,
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
        let states = self.resolve_states(&label_set, kind.as_ref(), &mut diagnostics);

        match kind {
            Some(kind) if diagnostics.is_empty() => {
                let relations = self.resolve_relations(&kind, &metadata, dependencies);
                Ok(ClassifiedArtifact {
                    kind,
                    target,
                    source,
                    updated_at,
                    states,
                    metadata,
                    relations,
                    labels: labels.to_vec(),
                })
            }
            _ => Err(ClassificationError::new(diagnostics)),
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
            // No labeled kind matched. If the target declares a default
            // (catch-all) kind — one with empty identifying labels — classify as
            // that, so raw human intake (an issue with no labels) is admitted as
            // a normal work item. Validation guarantees at most one default per
            // target, so this lookup is unambiguous.
            if let Some(default) = self.default_kind(target) {
                return Some(default.id.clone());
            }
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

    /// The default (catch-all) artifact kind for `target`, if one is declared.
    ///
    /// A default kind is one with no identifying labels: it admits any artifact
    /// of the target that no more specific labeled kind claims. Validation
    /// rejects more than one default per target, so the first match is the only
    /// match.
    fn default_kind(&self, target: ArtifactTarget) -> Option<&ValidatedArtifactKind> {
        self.workflow
            .artifact_kinds()
            .iter()
            .find(|kind| kind.target == target && kind.identifying_labels.is_empty())
    }

    fn resolve_relations(
        &self,
        source: &ArtifactKindId,
        metadata: &WorkflowMetadata,
        dependencies: &[ItemNumber],
    ) -> Vec<ClassifiedRelation> {
        let mut relations = Vec::new();
        self.push_metadata_relations(
            source,
            RelationKind::Parent,
            &metadata.parents,
            &mut relations,
        );
        self.push_metadata_relations(
            source,
            RelationKind::ProducedPr,
            &metadata.parents,
            &mut relations,
        );
        if dependencies.is_empty() {
            self.push_metadata_relations(
                source,
                RelationKind::Dependency,
                &metadata.dependencies,
                &mut relations,
            );
        } else {
            let mut dependency_targets: Vec<ArtifactRef> = dependencies
                .iter()
                .copied()
                .map(ArtifactRef::same_repo)
                .collect();
            dependency_targets.extend(
                metadata
                    .dependencies
                    .iter()
                    .filter(|target| !target.is_same_repo())
                    .cloned(),
            );
            self.push_metadata_relations(
                source,
                RelationKind::Dependency,
                &dependency_targets,
                &mut relations,
            );
        }
        relations
    }

    fn push_metadata_relations(
        &self,
        source: &ArtifactKindId,
        kind: RelationKind,
        targets: &[ArtifactRef],
        relations: &mut Vec<ClassifiedRelation>,
    ) {
        if targets.is_empty() {
            return;
        }
        let target_kinds: Vec<ArtifactKindId> = self
            .workflow
            .relations()
            .iter()
            .filter(|relation| relation.kind == kind && &relation.source == source)
            .map(|relation| relation.target.clone())
            .collect();
        if target_kinds.is_empty() {
            return;
        }
        for target in targets {
            relations.push(ClassifiedRelation {
                kind,
                source: source.clone(),
                target: target.clone(),
                target_kinds: target_kinds.clone(),
            });
        }
    }

    fn resolve_states(
        &self,
        labels: &HashSet<&str>,
        artifact: Option<&ArtifactKindId>,
        diagnostics: &mut Vec<ClassificationDiagnostic>,
    ) -> BTreeMap<StateDimensionId, Vec<StateId>> {
        let mut states = BTreeMap::new();
        for dimension in self.workflow.state_dimensions() {
            let active: Vec<_> = dimension
                .states
                .iter()
                .filter(|state| {
                    state
                        .label
                        .as_ref()
                        .is_some_and(|label| labels.contains(label.as_str()))
                })
                .collect();

            if active.is_empty() {
                continue;
            }
            let active_ids: Vec<StateId> = active.iter().map(|state| state.id.clone()).collect();
            if dimension.exclusive && active_ids.len() > 1 {
                diagnostics.push(ClassificationDiagnostic::ExclusiveStateConflict {
                    dimension: dimension.id.clone(),
                    states: active_ids.clone(),
                });
            }
            if let Some(artifact) = artifact {
                for state in active
                    .iter()
                    .filter(|state| !state.allows_artifact(artifact))
                {
                    diagnostics.push(ClassificationDiagnostic::StateNotAllowedForArtifact {
                        artifact: artifact.clone(),
                        dimension: dimension.id.clone(),
                        state: state.id.clone(),
                    });
                }
            }
            states.insert(dimension.id.clone(), active_ids);
        }
        states
    }
}
