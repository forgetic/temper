// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use temper_forge::{Issue, ItemNumber, PullRequest, RepositoryId};
use temper_protocol_worker::{
    ArtifactContextBundle, ArtifactContextDiagnostic, ArtifactContextDiagnosticCode,
    ArtifactIndexEntry, ArtifactReference, ArtifactRelation, ArtifactRelationType,
    ArtifactRepository, ArtifactSnapshot, ArtifactType,
};
use temper_workflow::{
    ArtifactSource, ArtifactTarget, ClassifiedArtifact, Classifier, RelationKind, ValidatedWorkflow,
};

use super::catalog::ConfiguredRepositoryCatalog;
use super::forge::ArtifactContextForge;
use super::{ArtifactContextError, ArtifactContextPolicy};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct ArtifactKey(pub String, pub u8, pub u64);

#[derive(Clone)]
pub(super) struct CollectedItem {
    pub snapshot: ArtifactSnapshot,
    pub classified: Option<ClassifiedArtifact>,
}

pub(super) struct Collection {
    pub primary: ArtifactKey,
    pub items: BTreeMap<ArtifactKey, CollectedItem>,
    pub relations: Vec<ArtifactRelation>,
    pub diagnostics: Vec<ArtifactContextDiagnostic>,
}

struct Pending {
    key: ArtifactKey,
    classified: ClassifiedArtifact,
    depth: usize,
    path: Vec<ArtifactKey>,
}

pub(super) async fn collect_lineage<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    repository_id: &RepositoryId,
    source: ArtifactSource,
    policy: ArtifactContextPolicy,
) -> Result<Collection, ArtifactContextError> {
    let repository = catalog.by_id(repository_id).cloned().ok_or_else(|| {
        ArtifactContextError::Primary(format!("repository `{repository_id}` is not configured"))
    })?;
    let primary_item = fetch(
        forge,
        repository_id,
        source_type(source),
        source_number(source),
    )
    .await
    .map_err(|error| ArtifactContextError::Primary(error.to_string()))?
    .ok_or_else(|| ArtifactContextError::Primary("coordinating artifact not found".into()))?;
    let primary_classified = classify(workflow, &primary_item)
        .map_err(|error| ArtifactContextError::Primary(error.to_string()))?;
    let primary_snapshot =
        primary_item.snapshot(repository, Some(primary_classified.kind.to_string()));
    let primary = key(&primary_snapshot.artifact);
    let mut collection = Collection {
        primary: primary.clone(),
        items: BTreeMap::from([(
            primary.clone(),
            CollectedItem {
                snapshot: primary_snapshot,
                classified: Some(primary_classified.clone()),
            },
        )]),
        relations: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut relations_seen = BTreeSet::new();
    let mut queue = VecDeque::from([Pending {
        key: primary.clone(),
        classified: primary_classified,
        depth: 0,
        path: vec![primary],
    }]);

    while let Some(pending) = queue.pop_front() {
        let mut parents: Vec<_> = pending
            .classified
            .relations
            .iter()
            .filter(|relation| relation.kind == RelationKind::Parent)
            .cloned()
            .collect();
        parents.sort_by_key(|relation| {
            (
                relation
                    .target
                    .repository_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                relation.target.number.get(),
            )
        });
        if pending.depth >= policy.lineage_depth && !parents.is_empty() {
            collection.diagnostics.push(diagnostic(
                ArtifactContextDiagnosticCode::DepthExceeded,
                "lineage depth limit reached",
                collection
                    .items
                    .get(&pending.key)
                    .map(|item| item.snapshot.artifact.clone()),
            ));
            continue;
        }
        for parent in parents {
            follow_parent(
                forge,
                catalog,
                workflow,
                policy,
                &pending,
                parent,
                &mut collection,
                &mut relations_seen,
                &mut queue,
            )
            .await;
        }
    }
    Ok(collection)
}

#[allow(clippy::too_many_arguments)]
async fn follow_parent<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    policy: ArtifactContextPolicy,
    pending: &Pending,
    relation: temper_workflow::ClassifiedRelation,
    collection: &mut Collection,
    relations_seen: &mut BTreeSet<(ArtifactKey, ArtifactKey)>,
    queue: &mut VecDeque<Pending>,
) {
    let source = collection.items[&pending.key].snapshot.artifact.clone();
    let repository_id = relation
        .target
        .repository_id
        .clone()
        .unwrap_or_else(|| RepositoryId::new(source.repository.id.clone()));
    let Some(repository) = catalog.by_id(&repository_id).cloned() else {
        collection.diagnostics.push(diagnostic(
            ArtifactContextDiagnosticCode::RepositoryNotAllowed,
            format!("parent repository `{repository_id}` is not configured"),
            Some(source),
        ));
        return;
    };
    let Some(artifact_type) = declared_target(workflow, &relation.target_kinds) else {
        collection.diagnostics.push(diagnostic(
            ArtifactContextDiagnosticCode::MalformedMetadata,
            "parent relation has ambiguous or inconsistent target kinds",
            Some(source),
        ));
        return;
    };
    let target_ref = reference(
        repository.clone(),
        artifact_type,
        relation.target.number.get(),
    );
    let target_key = key(&target_ref);
    if pending.path.contains(&target_key) {
        collection.diagnostics.push(diagnostic(
            ArtifactContextDiagnosticCode::CycleDetected,
            format!(
                "lineage cycle reaches {}#{}",
                repository.path, relation.target.number
            ),
            Some(source),
        ));
        return;
    }
    if relations_seen.insert((pending.key.clone(), target_key.clone())) {
        collection.relations.push(ArtifactRelation {
            relation_type: ArtifactRelationType::Parent,
            source,
            target: target_ref,
        });
    }
    if collection.items.contains_key(&target_key) {
        return;
    }
    if collection.items.len() >= policy.full_snapshots {
        collection.diagnostics.push(diagnostic(
            ArtifactContextDiagnosticCode::CountExceeded,
            "full snapshot limit reached while following lineage",
            collection
                .items
                .get(&pending.key)
                .map(|item| item.snapshot.artifact.clone()),
        ));
        return;
    }
    let fetched = fetch(forge, &repository_id, artifact_type, relation.target.number).await;
    let item = match fetched {
        Ok(Some(item)) => item,
        Ok(None) => {
            collection.diagnostics.push(diagnostic(
                ArtifactContextDiagnosticCode::MissingArtifact,
                format!(
                    "parent {}#{} does not exist",
                    repository.path, relation.target.number
                ),
                collection
                    .items
                    .get(&pending.key)
                    .map(|item| item.snapshot.artifact.clone()),
            ));
            return;
        }
        Err(error) => {
            collection.diagnostics.push(diagnostic(
                ArtifactContextDiagnosticCode::ForgeReadFailed,
                format!("parent read failed: {error}"),
                collection
                    .items
                    .get(&pending.key)
                    .map(|item| item.snapshot.artifact.clone()),
            ));
            return;
        }
    };
    let mut snapshot = item.snapshot(repository, None);
    if item.closed() {
        collection.diagnostics.push(diagnostic(
            ArtifactContextDiagnosticCode::ClosedAncestor,
            "closed ancestor retained in lineage",
            Some(snapshot.artifact.clone()),
        ));
    }
    let classified = match classify(workflow, &item) {
        Ok(classified) if relation.target_kinds.contains(&classified.kind) => {
            snapshot.workflow_kind = Some(classified.kind.to_string());
            Some(classified)
        }
        Ok(_) => {
            collection.diagnostics.push(diagnostic(
                ArtifactContextDiagnosticCode::MalformedMetadata,
                "parent kind is inconsistent with its relation declaration",
                Some(snapshot.artifact.clone()),
            ));
            None
        }
        Err(error) => {
            collection.diagnostics.push(diagnostic(
                ArtifactContextDiagnosticCode::MalformedMetadata,
                format!("parent classification failed: {error}"),
                Some(snapshot.artifact.clone()),
            ));
            None
        }
    };
    collection.items.insert(
        target_key.clone(),
        CollectedItem {
            snapshot,
            classified: classified.clone(),
        },
    );
    if let Some(classified) = classified {
        let mut path = pending.path.clone();
        path.push(target_key.clone());
        queue.push_back(Pending {
            key: target_key,
            classified,
            depth: pending.depth + 1,
            path,
        });
    }
}

pub(super) fn ordered_bundle(collection: Collection) -> ArtifactContextBundle {
    let mut order = Vec::new();
    let mut visited = BTreeSet::new();
    visit(
        &collection.primary,
        &collection.items,
        &collection.relations,
        &mut visited,
        &mut order,
    );
    for key in collection.items.keys() {
        visit(
            key,
            &collection.items,
            &collection.relations,
            &mut visited,
            &mut order,
        );
    }
    let primary = collection.items[&collection.primary].snapshot.clone();
    let mut bundle = ArtifactContextBundle::new(primary);
    bundle.lineage = order
        .into_iter()
        .filter(|key| *key != collection.primary)
        .map(|key| collection.items[&key].snapshot.clone())
        .collect();
    bundle.diagnostics = collection.diagnostics;
    bundle
}

fn visit(
    artifact_key: &ArtifactKey,
    items: &BTreeMap<ArtifactKey, CollectedItem>,
    relations: &[ArtifactRelation],
    visited: &mut BTreeSet<ArtifactKey>,
    order: &mut Vec<ArtifactKey>,
) {
    if !visited.insert(artifact_key.clone()) {
        return;
    }
    let mut parents: Vec<_> = relations
        .iter()
        .filter(|relation| key(&relation.source) == *artifact_key)
        .map(|relation| key(&relation.target))
        .filter(|parent| items.contains_key(parent))
        .collect();
    parents.sort();
    parents.dedup();
    for parent in parents {
        visit(&parent, items, relations, visited, order);
    }
    order.push(artifact_key.clone());
}

pub(super) enum ForgeItem {
    Issue(Issue),
    PullRequest(Box<PullRequest>),
}

impl ForgeItem {
    pub fn snapshot(
        &self,
        repository: ArtifactRepository,
        workflow_kind: Option<String>,
    ) -> ArtifactSnapshot {
        match self {
            Self::Issue(issue) => ArtifactSnapshot {
                artifact: reference(repository, ArtifactType::Issue, issue.number.get()),
                title: issue.title.clone(),
                body: issue.body.clone(),
                labels: sorted_labels(&issue.labels),
                state: format!("{:?}", issue.state).to_lowercase(),
                workflow_kind,
            },
            Self::PullRequest(pull_request) => ArtifactSnapshot {
                artifact: reference(
                    repository,
                    ArtifactType::PullRequest,
                    pull_request.number.get(),
                ),
                title: pull_request.title.clone(),
                body: pull_request.body.clone(),
                labels: sorted_labels(&pull_request.labels),
                state: format!("{:?}", pull_request.state).to_lowercase(),
                workflow_kind,
            },
        }
    }

    fn closed(&self) -> bool {
        match self {
            Self::Issue(issue) => issue.state != temper_forge::IssueState::Open,
            Self::PullRequest(pull_request) => {
                pull_request.state != temper_forge::PullRequestState::Open
            }
        }
    }
}

pub(super) async fn fetch<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    repository: &RepositoryId,
    artifact_type: ArtifactType,
    number: ItemNumber,
) -> temper_forge::ForgeResult<Option<ForgeItem>> {
    match artifact_type {
        ArtifactType::Issue => Ok(forge.issue(repository, number).await?.map(ForgeItem::Issue)),
        ArtifactType::PullRequest => Ok(forge
            .pull_request(repository, number)
            .await?
            .map(|pull_request| ForgeItem::PullRequest(Box::new(pull_request)))),
    }
}

pub(super) fn classify(
    workflow: &ValidatedWorkflow,
    item: &ForgeItem,
) -> Result<ClassifiedArtifact, temper_workflow::ClassificationError> {
    let classifier = Classifier::new(workflow);
    match item {
        ForgeItem::Issue(issue) => classifier.classify_issue(issue),
        ForgeItem::PullRequest(pull_request) => classifier.classify_pull_request(pull_request),
    }
}

pub(super) fn declared_target(
    workflow: &ValidatedWorkflow,
    kinds: &[temper_workflow::ArtifactKindId],
) -> Option<ArtifactType> {
    let targets: Vec<_> = kinds
        .iter()
        .filter_map(|kind| workflow.artifact_kind(kind))
        .map(|kind| match kind.target {
            ArtifactTarget::Issue => 0,
            ArtifactTarget::PullRequest => 1,
        })
        .collect();
    let targets: BTreeSet<_> = targets.into_iter().collect();
    if targets.len() != 1 {
        return None;
    }
    match targets.into_iter().next() {
        Some(0) => Some(ArtifactType::Issue),
        Some(1) => Some(ArtifactType::PullRequest),
        _ => None,
    }
}

pub(super) fn reference(
    repository: ArtifactRepository,
    artifact_type: ArtifactType,
    number: u64,
) -> ArtifactReference {
    ArtifactReference {
        repository,
        artifact_type,
        number,
    }
}

pub(super) fn key(reference: &ArtifactReference) -> ArtifactKey {
    ArtifactKey(
        reference.repository.id.clone(),
        match reference.artifact_type {
            ArtifactType::Issue => 0,
            ArtifactType::PullRequest => 1,
        },
        reference.number,
    )
}

pub(super) fn index(
    snapshot: &ArtifactSnapshot,
    snapshot_index: Option<usize>,
) -> ArtifactIndexEntry {
    ArtifactIndexEntry {
        artifact: snapshot.artifact.clone(),
        title: snapshot.title.clone(),
        state: snapshot.state.clone(),
        snapshot_index,
    }
}

pub(super) fn diagnostic(
    code: ArtifactContextDiagnosticCode,
    message: impl fmt::Display,
    source: Option<ArtifactReference>,
) -> ArtifactContextDiagnostic {
    ArtifactContextDiagnostic {
        code,
        message: message.to_string(),
        source,
    }
}

fn source_number(source: ArtifactSource) -> ItemNumber {
    match source {
        ArtifactSource::Issue { number } | ArtifactSource::PullRequest { number } => number,
    }
}

fn source_type(source: ArtifactSource) -> ArtifactType {
    match source {
        ArtifactSource::Issue { .. } => ArtifactType::Issue,
        ArtifactSource::PullRequest { .. } => ArtifactType::PullRequest,
    }
}

fn sorted_labels(labels: &[String]) -> Vec<String> {
    let mut labels = labels.to_vec();
    labels.sort();
    labels.dedup();
    labels
}
