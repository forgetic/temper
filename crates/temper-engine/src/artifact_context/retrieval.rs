// SPDX-License-Identifier: MPL-2.0

//! Bounded, transport-neutral on-demand Forge reads.

use std::collections::{BTreeMap, BTreeSet};

mod response;
mod validation;

pub use validation::validate_context_operation;

use response::{
    bounded_comment, enforce_item_response_bound, enforce_related_response_bound, load_comments,
    load_item, resolve_repository, truncate_utf8, validate_identity,
};

use temper_forge::{
    IssueQuery, ItemListDetails, ItemNumber, ItemSort, ItemSortField, PullRequestQuery,
    RepositoryId, SortDirection,
};
use temper_protocol_context::{
    ArtifactContextTruncation, ArtifactReference, ArtifactSnapshot, ArtifactType,
    ForgeContextErrorCode, ForgeContextOperation, ForgeContextResult, ForgeGetItemOperation,
    ForgeGetItemResult, ForgeListRelatedOperation, ForgeListRelatedResult, ForgeRelatedEdge,
    ForgeRelationType,
};
use temper_workflow::{ArtifactTarget, ClassifiedArtifact, RelationKind, ValidatedWorkflow};

use super::catalog::ConfiguredRepositoryCatalog;
use super::forge::ArtifactContextForge;
use super::lineage::{
    ArtifactKey, ForgeItem, classify, declared_target, fetch, index, key, reference,
};
use super::markdown::{ArtifactTypeKey, MarkdownReference, references};

pub const DEFAULT_RELATED_DEPTH: usize = 1;
pub const MAX_RELATED_DEPTH: usize = 2;
pub const DEFAULT_RELATED_RESULTS: usize = 50;
pub const MAX_RELATED_RESULTS: usize = 50;
pub const MAX_ITEM_COMMENTS: usize = 20;
pub const MAX_COMMENT_BYTES: usize = 8 * 1024;
pub const MAX_ITEM_BODY_BYTES: usize = 256 * 1024;
pub const MAX_FORGE_RESPONSE_BYTES: usize = 512 * 1024;
const INVERSE_SCAN_LIMIT: usize = 100;
// Leaves room for the tagged result plus bounded worker/job transport identity.
const MAX_INNER_RESPONSE_BYTES: usize = MAX_FORGE_RESPONSE_BYTES - 2048;

/// Read-only artifact-context service shared by transport adapters.
///
/// Its only Forge capability is [`ArtifactContextForge`], whose method set has
/// no mutation operation.
pub struct ArtifactContextService<'a, F: ArtifactContextForge + ?Sized> {
    forge: &'a F,
    catalog: &'a ConfiguredRepositoryCatalog,
    workflow: &'a ValidatedWorkflow,
}

impl<'a, F: ArtifactContextForge + ?Sized> ArtifactContextService<'a, F> {
    pub const fn new(
        forge: &'a F,
        catalog: &'a ConfiguredRepositoryCatalog,
        workflow: &'a ValidatedWorkflow,
    ) -> Self {
        Self {
            forge,
            catalog,
            workflow,
        }
    }

    /// Executes one closed-vocabulary read operation.
    pub async fn execute(
        &self,
        operation: ForgeContextOperation,
    ) -> Result<ForgeContextResult, ForgeContextErrorCode> {
        validate_context_operation(&operation, self.catalog)?;
        match operation {
            ForgeContextOperation::ForgeGetItem(operation) => self
                .forge_get_item(operation)
                .await
                .map(ForgeContextResult::Item),
            ForgeContextOperation::ForgeListRelated(operation) => self
                .forge_list_related(operation)
                .await
                .map(ForgeContextResult::Related),
        }
    }

    /// Returns one configured-repository item and optional bounded comments.
    pub async fn forge_get_item(
        &self,
        operation: ForgeGetItemOperation,
    ) -> Result<ForgeGetItemResult, ForgeContextErrorCode> {
        validate_identity(&operation.repo, operation.number)?;
        let (repository_id, repository) = resolve_repository(self.catalog, &operation.repo)?;
        let item = load_item(
            self.forge,
            &repository_id,
            operation.number,
            operation.artifact_type,
        )
        .await?;
        let mut item_snapshot = item.snapshot(repository);
        let mut truncation = ArtifactContextTruncation::default();
        if truncate_utf8(&mut item_snapshot.body, MAX_ITEM_BODY_BYTES) {
            truncation.content_truncated = true;
        }

        let mut comments = if operation.include_comments {
            load_comments(self.forge, &item).await?
        } else {
            Vec::new()
        };
        comments.sort_by(|left, right| {
            (left.created_at, left.id.to_string()).cmp(&(right.created_at, right.id.to_string()))
        });
        if comments.len() > MAX_ITEM_COMMENTS {
            comments.truncate(MAX_ITEM_COMMENTS);
            truncation.count_exceeded = true;
        }
        let comments = comments
            .into_iter()
            .map(|comment| bounded_comment(comment, &mut truncation))
            .collect();
        let mut result = ForgeGetItemResult {
            item: item_snapshot,
            comments,
            truncation,
        };
        enforce_item_response_bound(&mut result)?;
        Ok(result)
    }

    /// Traverses a selected relation set under strict depth and result limits.
    pub async fn forge_list_related(
        &self,
        operation: ForgeListRelatedOperation,
    ) -> Result<ForgeListRelatedResult, ForgeContextErrorCode> {
        validate_identity(&operation.repo, operation.number)?;
        if operation.relations.is_empty() {
            return Err(ForgeContextErrorCode::InvalidRequest);
        }
        if operation.relations.len() > 7 {
            return Err(ForgeContextErrorCode::LimitExceeded);
        }
        let depth = operation.depth.unwrap_or(DEFAULT_RELATED_DEPTH);
        let limit = operation.limit.unwrap_or(DEFAULT_RELATED_RESULTS);
        if depth == 0 || depth > MAX_RELATED_DEPTH || limit == 0 || limit > MAX_RELATED_RESULTS {
            return Err(ForgeContextErrorCode::LimitExceeded);
        }

        let (repository_id, repository) = resolve_repository(self.catalog, &operation.repo)?;
        let root_item = load_item(
            self.forge,
            &repository_id,
            operation.number,
            operation.artifact_type,
        )
        .await?;
        let root_snapshot = root_item.snapshot(repository);
        let root_key = key(&root_snapshot.artifact);
        let root = root_snapshot.artifact.clone();
        let requested: BTreeSet<_> = operation.relations.into_iter().collect();
        let mut nodes = BTreeMap::from([(
            root_key.clone(),
            Node {
                item: root_item,
                snapshot: root_snapshot,
            },
        )]);
        let mut visited = BTreeSet::from([root_key.clone()]);
        let mut frontier = vec![root_key];
        let mut edges = BTreeMap::new();
        let mut truncation = ArtifactContextTruncation::default();

        for _ in 0..depth {
            if frontier.is_empty() {
                break;
            }
            frontier.sort();
            let mut discoveries = BTreeMap::<ArtifactKey, Discovery>::new();
            let mut round_edges = BTreeMap::new();
            for source_key in &frontier {
                let source = nodes
                    .get(source_key)
                    .expect("frontier contains loaded nodes");
                let found = discover(
                    self.forge,
                    self.catalog,
                    self.workflow,
                    source,
                    &requested,
                    &mut truncation,
                )
                .await?;
                for discovery in found {
                    let edge_key = edge_identity(&discovery.edge);
                    round_edges
                        .entry(edge_key)
                        .or_insert(discovery.edge.clone());
                    discoveries
                        .entry(key(&discovery.node.snapshot.artifact))
                        .or_insert(discovery);
                }
            }

            let remaining = limit.saturating_sub(visited.len().saturating_sub(1));
            let new_count = discoveries
                .keys()
                .filter(|candidate| !visited.contains(*candidate))
                .count();
            if new_count > remaining {
                truncation.count_exceeded = true;
            }
            let selected: BTreeSet<_> = discoveries
                .keys()
                .filter(|candidate| !visited.contains(*candidate))
                .take(remaining)
                .cloned()
                .collect();
            let mut next = Vec::new();
            for (candidate_key, discovery) in discoveries {
                if visited.contains(&candidate_key) {
                    continue;
                }
                if !selected.contains(&candidate_key) {
                    continue;
                }
                visited.insert(candidate_key.clone());
                next.push(candidate_key.clone());
                nodes.insert(candidate_key, discovery.node);
            }
            for (edge_identity, edge) in round_edges {
                if visited.contains(&edge_identity.1) && visited.contains(&edge_identity.2) {
                    edges.entry(edge_identity).or_insert(edge);
                }
            }
            frontier = next;
            if remaining == 0 || truncation.count_exceeded {
                break;
            }
        }

        let mut items = Vec::new();
        for (item_key, node) in &nodes {
            if *item_key != key(&root) {
                items.push(index(&node.snapshot, None));
            }
        }
        let edges = edges.into_values().collect();
        let mut result = ForgeListRelatedResult {
            root,
            items,
            edges,
            truncation,
        };
        enforce_related_response_bound(&mut result)?;
        Ok(result)
    }
}

struct Node {
    item: ForgeItem,
    snapshot: ArtifactSnapshot,
}

struct Discovery {
    node: Node,
    edge: ForgeRelatedEdge,
}

type EdgeIdentity = (ForgeRelationType, ArtifactKey, ArtifactKey);

async fn discover<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    source: &Node,
    requested: &BTreeSet<ForgeRelationType>,
    truncation: &mut ArtifactContextTruncation,
) -> Result<Vec<Discovery>, ForgeContextErrorCode> {
    let mut output = Vec::new();
    let needs_typed = requested.iter().any(|relation| {
        matches!(
            relation,
            ForgeRelationType::Parent
                | ForgeRelationType::Child
                | ForgeRelationType::Dependency
                | ForgeRelationType::Dependent
                | ForgeRelationType::ProducedPr
        )
    });
    let classified = if needs_typed {
        Some(classify(workflow, &source.item).map_err(|_| ForgeContextErrorCode::InvalidRequest)?)
    } else {
        None
    };

    if let Some(classified) = classified.as_ref() {
        output
            .extend(outbound_typed(forge, catalog, workflow, source, classified, requested).await?);
        for relation in requested {
            let kind = match relation {
                ForgeRelationType::Child => Some(RelationKind::Parent),
                ForgeRelationType::Dependent => Some(RelationKind::Dependency),
                ForgeRelationType::ProducedPr => Some(RelationKind::ProducedPr),
                _ => None,
            };
            if let Some(kind) = kind {
                output.extend(
                    inverse_typed(
                        forge, catalog, workflow, source, classified, *relation, kind, truncation,
                    )
                    .await?,
                );
            }
        }
    }
    if requested.contains(&ForgeRelationType::BodyReference) {
        output.extend(outbound_references(forge, catalog, source).await?);
    }
    if requested.contains(&ForgeRelationType::ReferencedBy) {
        output.extend(inverse_references(forge, catalog, workflow, source, truncation).await?);
    }
    output.sort_by_key(|discovery| edge_identity(&discovery.edge));
    output.dedup_by_key(|discovery| edge_identity(&discovery.edge));
    Ok(output)
}

async fn outbound_typed<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    source: &Node,
    classified: &ClassifiedArtifact,
    requested: &BTreeSet<ForgeRelationType>,
) -> Result<Vec<Discovery>, ForgeContextErrorCode> {
    let mut output = Vec::new();
    for relation in &classified.relations {
        let relation_type = match relation.kind {
            RelationKind::Parent => ForgeRelationType::Parent,
            RelationKind::Dependency => ForgeRelationType::Dependency,
            RelationKind::ProducedPr => ForgeRelationType::ProducedPr,
        };
        if !requested.contains(&relation_type) {
            continue;
        }
        let repository_id =
            relation.target.repository_id.clone().unwrap_or_else(|| {
                RepositoryId::new(source.snapshot.artifact.repository.id.clone())
            });
        let Some(repository) = catalog.by_id(&repository_id).cloned() else {
            continue;
        };
        let Some(artifact_type) = declared_target(workflow, &relation.target_kinds) else {
            return Err(ForgeContextErrorCode::InvalidRequest);
        };
        let Some(item) = fetch(forge, &repository_id, artifact_type, relation.target.number)
            .await
            .map_err(|_| ForgeContextErrorCode::ForgeUnavailable)?
        else {
            continue;
        };
        let snapshot = item.snapshot(repository);
        output.push(Discovery {
            edge: ForgeRelatedEdge {
                relation: relation_type,
                source: source.snapshot.artifact.clone(),
                target: snapshot.artifact.clone(),
            },
            node: Node { item, snapshot },
        });
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
async fn inverse_typed<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    target: &Node,
    target_classified: &ClassifiedArtifact,
    relation_type: ForgeRelationType,
    relation_kind: RelationKind,
    truncation: &mut ArtifactContextTruncation,
) -> Result<Vec<Discovery>, ForgeContextErrorCode> {
    let source_kinds: Vec<_> = workflow
        .relations()
        .iter()
        .filter(|relation| {
            relation.kind == relation_kind && relation.target == target_classified.kind
        })
        .filter_map(|relation| workflow.artifact_kind(&relation.source))
        .collect();
    let mut candidates = Vec::new();
    for (repository_id, repository) in catalog.repositories() {
        for source_kind in &source_kinds {
            let items = scan_kind(
                forge,
                &repository_id,
                source_kind.target,
                source_kind
                    .identifying_labels
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                truncation,
            )
            .await?;
            for item in items {
                let Ok(classified) = classify(workflow, &item) else {
                    continue;
                };
                let snapshot = item.snapshot(repository.clone());
                if !classified.relations.iter().any(|relation| {
                    relation.kind == relation_kind
                        && relation_targets(
                            relation,
                            &snapshot.artifact,
                            &target.snapshot.artifact,
                            workflow,
                        )
                }) {
                    continue;
                }
                candidates.push(Discovery {
                    edge: ForgeRelatedEdge {
                        relation: relation_type,
                        source: snapshot.artifact.clone(),
                        target: target.snapshot.artifact.clone(),
                    },
                    node: Node { item, snapshot },
                });
            }
        }
    }
    Ok(candidates)
}

fn relation_targets(
    relation: &temper_workflow::ClassifiedRelation,
    source: &ArtifactReference,
    expected: &ArtifactReference,
    workflow: &ValidatedWorkflow,
) -> bool {
    let repository = relation
        .target
        .repository_id
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| source.repository.id.clone());
    repository == expected.repository.id
        && relation.target.number.get() == expected.number
        && declared_target(workflow, &relation.target_kinds) == Some(expected.artifact_type)
}

async fn outbound_references<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    source: &Node,
) -> Result<Vec<Discovery>, ForgeContextErrorCode> {
    let mut output = Vec::new();
    for found in references(&source.snapshot.body, catalog.forge_url()) {
        let Some((repository_id, artifact)) = markdown_target(catalog, &source.snapshot, &found)
        else {
            continue;
        };
        if key(&artifact) == key(&source.snapshot.artifact) {
            continue;
        }
        let Some(item) = fetch(
            forge,
            &repository_id,
            artifact.artifact_type,
            ItemNumber::new(artifact.number),
        )
        .await
        .map_err(|_| ForgeContextErrorCode::ForgeUnavailable)?
        else {
            continue;
        };
        let snapshot = item.snapshot(artifact.repository);
        output.push(Discovery {
            edge: ForgeRelatedEdge {
                relation: ForgeRelationType::BodyReference,
                source: source.snapshot.artifact.clone(),
                target: snapshot.artifact.clone(),
            },
            node: Node { item, snapshot },
        });
    }
    Ok(output)
}

async fn inverse_references<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    catalog: &ConfiguredRepositoryCatalog,
    workflow: &ValidatedWorkflow,
    target: &Node,
    truncation: &mut ArtifactContextTruncation,
) -> Result<Vec<Discovery>, ForgeContextErrorCode> {
    let mut output = Vec::new();
    for (repository_id, repository) in catalog.repositories() {
        for kind in workflow.artifact_kinds() {
            let items = scan_kind(
                forge,
                &repository_id,
                kind.target,
                kind.identifying_labels
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                truncation,
            )
            .await?;
            for item in items {
                if classify(workflow, &item).is_err() {
                    continue;
                }
                let snapshot = item.snapshot(repository.clone());
                let verified = references(&snapshot.body, catalog.forge_url())
                    .iter()
                    .filter_map(|found| markdown_target(catalog, &snapshot, found))
                    .any(|(_, artifact)| key(&artifact) == key(&target.snapshot.artifact));
                if !verified || key(&snapshot.artifact) == key(&target.snapshot.artifact) {
                    continue;
                }
                output.push(Discovery {
                    edge: ForgeRelatedEdge {
                        relation: ForgeRelationType::ReferencedBy,
                        source: snapshot.artifact.clone(),
                        target: target.snapshot.artifact.clone(),
                    },
                    node: Node { item, snapshot },
                });
            }
        }
    }
    Ok(output)
}

async fn scan_kind<F: ArtifactContextForge + ?Sized>(
    forge: &F,
    repository: &RepositoryId,
    target: ArtifactTarget,
    labels: Vec<String>,
    truncation: &mut ArtifactContextTruncation,
) -> Result<Vec<ForgeItem>, ForgeContextErrorCode> {
    let sort = Some(ItemSort {
        field: ItemSortField::Number,
        direction: SortDirection::Asc,
    });
    let items = match target {
        ArtifactTarget::Issue => forge
            .issues(
                repository,
                IssueQuery {
                    labels,
                    sort,
                    limit: Some(INVERSE_SCAN_LIMIT),
                    details: ItemListDetails::full(),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| ForgeContextErrorCode::ForgeUnavailable)?
            .into_iter()
            .map(ForgeItem::Issue)
            .collect::<Vec<_>>(),
        ArtifactTarget::PullRequest => forge
            .pull_requests(
                repository,
                PullRequestQuery {
                    labels,
                    sort,
                    limit: Some(INVERSE_SCAN_LIMIT),
                    details: ItemListDetails::full(),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| ForgeContextErrorCode::ForgeUnavailable)?
            .into_iter()
            .map(|item| ForgeItem::PullRequest(Box::new(item)))
            .collect::<Vec<_>>(),
    };
    if items.len() >= INVERSE_SCAN_LIMIT {
        truncation.count_exceeded = true;
    }
    Ok(items.into_iter().take(INVERSE_SCAN_LIMIT).collect())
}

fn markdown_target(
    catalog: &ConfiguredRepositoryCatalog,
    source: &ArtifactSnapshot,
    found: &MarkdownReference,
) -> Option<(RepositoryId, ArtifactReference)> {
    let (repository_id, repository) = match found.path.as_deref() {
        Some(path) => catalog
            .by_path(path)
            .map(|(id, repository)| (id, repository.clone()))?,
        None => {
            let id = RepositoryId::new(source.artifact.repository.id.clone());
            (id.clone(), catalog.by_id(&id)?.clone())
        }
    };
    let artifact_type = match found.artifact_type {
        ArtifactTypeKey::Issue => ArtifactType::Issue,
        ArtifactTypeKey::PullRequest => ArtifactType::PullRequest,
    };
    Some((
        repository_id,
        reference(repository, artifact_type, found.number),
    ))
}

fn edge_identity(edge: &ForgeRelatedEdge) -> EdgeIdentity {
    (edge.relation, key(&edge.source), key(&edge.target))
}

#[cfg(test)]
mod tests;
