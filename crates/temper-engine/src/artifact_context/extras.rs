// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use temper_forge::{ItemListDetails, ItemNumber, PullRequestQuery, RepositoryId};
use temper_protocol_worker::{
    ArtifactContextDiagnostic, ArtifactContextDiagnosticCode, ArtifactReference,
    ArtifactRelationType, ArtifactSummary, ArtifactType,
};
use temper_workflow::{ClassifiedArtifact, RelationKind, ValidatedWorkflow};

use super::catalog::ConfiguredRepositoryCatalog;
use super::forge::ArtifactContextForge;
use super::lineage::{
    ArtifactKey, Collection, classify, declared_target, diagnostic, fetch, key, reference,
};
use super::markdown::{ArtifactTypeKey, references};

pub(super) struct Extras {
    pub summaries: Vec<ArtifactSummary>,
    pub diagnostics: Vec<ArtifactContextDiagnostic>,
}

struct Candidate {
    summary: ArtifactSummary,
}

pub(super) async fn collect_validation_aggregates<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    collection: &Collection,
) -> Extras {
    let mut diagnostics = Vec::new();
    let mut candidates =
        validation_candidates(forge, catalog, workflow, collection, &mut diagnostics).await;
    normalize_candidates(&mut candidates);
    extras(candidates, diagnostics)
}

pub(super) async fn collect_references<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    collection: &Collection,
) -> Extras {
    let mut diagnostics = Vec::new();
    let mut candidates =
        reference_candidates(forge, catalog, workflow, collection, &mut diagnostics).await;
    normalize_candidates(&mut candidates);
    extras(candidates, diagnostics)
}

fn normalize_candidates(candidates: &mut Vec<Candidate>) {
    candidates.sort_by_key(|candidate| key(&candidate.summary.artifact));
    candidates.dedup_by_key(|candidate| key(&candidate.summary.artifact));
}

fn extras(candidates: Vec<Candidate>, diagnostics: Vec<ArtifactContextDiagnostic>) -> Extras {
    Extras {
        summaries: candidates
            .into_iter()
            .map(|candidate| candidate.summary)
            .collect(),
        diagnostics,
    }
}

fn summary(
    snapshot: &temper_protocol_worker::ArtifactSnapshot,
    relation_type: ArtifactRelationType,
    source: ArtifactReference,
) -> ArtifactSummary {
    ArtifactSummary {
        artifact: snapshot.artifact.clone(),
        title: snapshot.title.clone(),
        labels: snapshot.labels.clone(),
        state: snapshot.state.clone(),
        workflow_kind: snapshot.workflow_kind.clone(),
        relation_type,
        source,
    }
}

async fn validation_candidates<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    collection: &Collection,
    diagnostics: &mut Vec<ArtifactContextDiagnostic>,
) -> Vec<Candidate> {
    let Some(primary) = collection.items.get(&collection.primary) else {
        return Vec::new();
    };
    let Some(classified) = primary.classified.as_ref() else {
        return Vec::new();
    };
    let mut output = Vec::new();
    for relation in classified
        .relations
        .iter()
        .filter(|relation| relation.kind == RelationKind::Dependency)
    {
        let source = primary.snapshot.artifact.clone();
        let repository_id = relation
            .target
            .repository_id
            .clone()
            .unwrap_or_else(|| RepositoryId::new(source.repository.id.clone()));
        let Some(repository) = catalog.by_id(&repository_id).cloned() else {
            diagnostics.push(diagnostic(
                ArtifactContextDiagnosticCode::RepositoryNotAllowed,
                format!("dependency repository `{repository_id}` is not configured"),
                Some(source),
            ));
            continue;
        };
        let Some(artifact_type) = declared_target(workflow, &relation.target_kinds) else {
            diagnostics.push(diagnostic(
                ArtifactContextDiagnosticCode::MalformedMetadata,
                "dependency relation has ambiguous target kinds",
                Some(source),
            ));
            continue;
        };
        match fetch(forge, &repository_id, artifact_type, relation.target.number).await {
            Ok(Some(item)) => {
                let classified_dependency = classify(workflow, &item);
                let mut snapshot = item.snapshot(repository, None);
                if let Ok(dependency) = &classified_dependency {
                    snapshot.workflow_kind = Some(dependency.kind.to_string());
                }
                let dependency_ref = snapshot.artifact.clone();
                output.push(Candidate {
                    summary: summary(
                        &snapshot,
                        ArtifactRelationType::Dependency,
                        primary.snapshot.artifact.clone(),
                    ),
                });
                match classified_dependency {
                    Ok(dependency) => {
                        output.extend(
                            implementation_prs(
                                forge,
                                catalog,
                                workflow,
                                &dependency,
                                &dependency_ref,
                                diagnostics,
                            )
                            .await,
                        );
                    }
                    Err(error) => diagnostics.push(diagnostic(
                        ArtifactContextDiagnosticCode::MalformedMetadata,
                        format!("validation dependency classification failed: {error}"),
                        Some(dependency_ref),
                    )),
                }
            }
            Ok(None) => diagnostics.push(diagnostic(
                ArtifactContextDiagnosticCode::MissingArtifact,
                "declared validation dependency is missing",
                Some(source),
            )),
            Err(error) => diagnostics.push(diagnostic(
                ArtifactContextDiagnosticCode::ForgeReadFailed,
                format!("validation dependency read failed: {error}"),
                Some(source),
            )),
        }
    }
    output
}

async fn implementation_prs<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    dependency: &ClassifiedArtifact,
    dependency_ref: &ArtifactReference,
    diagnostics: &mut Vec<ArtifactContextDiagnostic>,
) -> Vec<Candidate> {
    let source_kinds: Vec<_> = workflow
        .relations()
        .iter()
        .filter(|relation| {
            relation.kind == RelationKind::ProducedPr && relation.target == dependency.kind
        })
        .filter_map(|relation| workflow.artifact_kind(&relation.source))
        .filter(|kind| kind.target == temper_workflow::ArtifactTarget::PullRequest)
        .collect();
    if source_kinds.is_empty() {
        return Vec::new();
    }
    let mut output = Vec::new();
    for (repository_id, repository) in catalog.repositories() {
        for kind in &source_kinds {
            let query = PullRequestQuery {
                labels: kind
                    .identifying_labels
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                limit: Some(101),
                details: ItemListDetails::summary(),
                ..Default::default()
            };
            let pull_requests = match forge.pull_requests(&repository_id, query).await {
                Ok(items) => items,
                Err(error) => {
                    diagnostics.push(diagnostic(
                        ArtifactContextDiagnosticCode::ForgeReadFailed,
                        format!(
                            "implementation PR list failed in {}: {error}",
                            repository.path
                        ),
                        Some(dependency_ref.clone()),
                    ));
                    continue;
                }
            };
            for pull_request in pull_requests {
                let classified = match temper_workflow::Classifier::new(workflow)
                    .classify_pull_request(&pull_request)
                {
                    Ok(classified) => classified,
                    Err(error) => {
                        diagnostics.push(diagnostic(
                            ArtifactContextDiagnosticCode::MalformedMetadata,
                            format!(
                                "implementation PR #{} classification failed: {error}",
                                pull_request.number
                            ),
                            Some(dependency_ref.clone()),
                        ));
                        continue;
                    }
                };
                let verified = classified.relations.iter().any(|relation| {
                    relation.kind == RelationKind::ProducedPr
                        && relation.target.number.get() == dependency_ref.number
                        && relation
                            .target
                            .repository_id
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_else(|| repository_id.to_string())
                            == dependency_ref.repository.id
                });
                if !verified {
                    continue;
                }
                let snapshot = super::lineage::ForgeItem::PullRequest(Box::new(pull_request))
                    .snapshot(repository.clone(), Some(classified.kind.to_string()));
                output.push(Candidate {
                    summary: summary(
                        &snapshot,
                        ArtifactRelationType::Related,
                        dependency_ref.clone(),
                    ),
                });
            }
        }
    }
    output
}

async fn reference_candidates<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    collection: &Collection,
    diagnostics: &mut Vec<ArtifactContextDiagnostic>,
) -> Vec<Candidate> {
    let mut requests = BTreeMap::<ArtifactKey, (ArtifactReference, ArtifactReference)>::new();
    for item in collection.items.values() {
        for found in references(&item.snapshot.body, catalog.forge_url()) {
            let resolved = match found.path.as_deref() {
                Some(path) => catalog.by_path(path).map(|(id, repo)| (id, repo.clone())),
                None => {
                    let id = RepositoryId::new(item.snapshot.artifact.repository.id.clone());
                    catalog.by_id(&id).cloned().map(|repo| (id, repo))
                }
            };
            let Some((_repository_id, repository)) = resolved else {
                diagnostics.push(diagnostic(
                    ArtifactContextDiagnosticCode::RepositoryNotAllowed,
                    format!(
                        "markdown reference repository is not configured: {:?}",
                        found.path
                    ),
                    Some(item.snapshot.artifact.clone()),
                ));
                continue;
            };
            let artifact_type = match found.artifact_type {
                ArtifactTypeKey::Issue => ArtifactType::Issue,
                ArtifactTypeKey::PullRequest => ArtifactType::PullRequest,
            };
            let target = reference(repository, artifact_type, found.number);
            if collection.items.contains_key(&key(&target)) {
                continue;
            }
            requests
                .entry(key(&target))
                .or_insert((item.snapshot.artifact.clone(), target));
        }
    }
    let mut output = Vec::new();
    for (_key, (source, target)) in requests {
        let repository_id = RepositoryId::new(target.repository.id.clone());
        match fetch(
            forge,
            &repository_id,
            target.artifact_type,
            ItemNumber::new(target.number),
        )
        .await
        {
            Ok(Some(item)) => {
                let workflow_kind = classify(workflow, &item)
                    .ok()
                    .map(|classified| classified.kind.to_string());
                let snapshot = item.snapshot(target.repository.clone(), workflow_kind);
                output.push(Candidate {
                    summary: summary(&snapshot, ArtifactRelationType::Related, source),
                });
            }
            Ok(None) => diagnostics.push(diagnostic(
                ArtifactContextDiagnosticCode::MissingArtifact,
                format!(
                    "markdown reference {}#{} is missing",
                    target.repository.path, target.number
                ),
                Some(source),
            )),
            Err(error) => diagnostics.push(diagnostic(
                ArtifactContextDiagnosticCode::ForgeReadFailed,
                format!("markdown reference read failed: {error}"),
                Some(source),
            )),
        }
    }
    output
}
