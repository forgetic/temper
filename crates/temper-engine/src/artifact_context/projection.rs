// SPDX-License-Identifier: MPL-2.0

//! Canonical projection from raw Forge artifacts into transport-safe context.
//!
//! This is the only boundary that is allowed to copy a Forge body into an
//! [`ArtifactSnapshot`]. It removes valid managed workflow metadata before any
//! body or bundle bound is applied and projects only the compact, non-recursive
//! workflow fields owned by the protocol contract.

use std::collections::{BTreeMap, BTreeSet};

use temper_protocol_context::{
    ArtifactContextBundle, ArtifactReference, ArtifactRepository, ArtifactSnapshot, ArtifactType,
    ArtifactWorkflowContext, WorkflowArtifactReference, WorkflowChildIdentity,
};
use temper_workflow::{ArtifactRef, WorkflowMetadata, split_metadata_block};

/// Raw Forge fields needed to construct one context snapshot.
pub(crate) struct SnapshotInput {
    pub repository: ArtifactRepository,
    pub artifact_type: ArtifactType,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub state: String,
    pub workflow_kind: Option<String>,
}

/// Projects a raw Forge artifact into authored content plus compact workflow
/// context. Parse failures deliberately retain the complete raw body.
pub(crate) fn project_snapshot(input: SnapshotInput) -> ArtifactSnapshot {
    let SnapshotInput {
        repository,
        artifact_type,
        number,
        title,
        body,
        mut labels,
        state,
        workflow_kind,
    } = input;
    labels.sort();
    labels.dedup();

    let source_repository_id = repository.id.clone();
    let (body, metadata) = match split_metadata_block(&body) {
        Ok(parts) => parts,
        Err(_) => (body, None),
    };
    let metadata_kind = metadata
        .as_ref()
        .and_then(|metadata| metadata.kind.as_ref())
        .and_then(|kind| normalized(Some(kind.to_string())));
    let workflow_kind = metadata_kind.or_else(|| normalized(workflow_kind));
    let workflow = project_workflow(metadata, &source_repository_id, workflow_kind.clone());

    ArtifactSnapshot {
        artifact: ArtifactReference {
            repository,
            artifact_type,
            number,
        },
        title,
        body,
        labels,
        state,
        workflow_kind,
        workflow,
    }
}

fn project_workflow(
    metadata: Option<WorkflowMetadata>,
    source_repository_id: &str,
    kind: Option<String>,
) -> Option<ArtifactWorkflowContext> {
    let mut projected = match metadata {
        Some(mut metadata) => {
            let children = persisted_children(std::mem::take(&mut metadata.create_issue_intents));
            ArtifactWorkflowContext {
                kind: None,
                parents: normalize_references(metadata.parents, source_repository_id),
                dependencies: normalize_references(metadata.dependencies, source_repository_id),
                target_branch: normalized(metadata.target_branch),
                correlation_key: normalized(metadata.correlation_key),
                children,
            }
        }
        None => ArtifactWorkflowContext::default(),
    };
    projected.kind = kind;
    (projected != ArtifactWorkflowContext::default()).then_some(projected)
}

fn normalize_references(
    references: Vec<ArtifactRef>,
    source_repository_id: &str,
) -> Vec<WorkflowArtifactReference> {
    references
        .into_iter()
        .map(|reference| WorkflowArtifactReference {
            repository_id: reference
                .repository_id
                .map(|repository| repository.to_string())
                .unwrap_or_else(|| source_repository_id.to_string()),
            number: reference.number.get(),
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn persisted_children(
    intents: BTreeMap<String, temper_workflow::CreateIssuesIntent>,
) -> Vec<WorkflowChildIdentity> {
    let mut children = BTreeMap::<(String, u64), String>::new();
    for intent in intents.into_values() {
        for child in intent.children {
            let Some(number) = child.number else {
                continue;
            };
            let key = (child.repository_id.to_string(), number.get());
            children
                .entry(key)
                .and_modify(|title| {
                    if child.title < *title {
                        *title = child.title.clone();
                    }
                })
                .or_insert(child.title);
        }
    }
    children
        .into_iter()
        .map(|((repository_id, number), title)| WorkflowChildIdentity {
            repository_id,
            number,
            title,
            state: None,
        })
        .collect()
}

fn normalized(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Attaches child state strictly from artifacts already retained in the
/// bounded startup collection. No Forge access is performed here.
pub(super) fn attach_available_child_states(bundle: &mut ArtifactContextBundle) {
    let mut states = BTreeMap::<(String, u64), String>::new();
    for snapshot in std::iter::once(&bundle.primary).chain(bundle.lineage.iter()) {
        if snapshot.artifact.artifact_type == ArtifactType::Issue {
            states.insert(
                (
                    snapshot.artifact.repository.id.clone(),
                    snapshot.artifact.number,
                ),
                snapshot.state.clone(),
            );
        }
    }
    for summary in bundle
        .validation_scope
        .iter()
        .chain(bundle.optional_references.iter())
    {
        if summary.artifact.artifact_type == ArtifactType::Issue {
            states.insert(
                (
                    summary.artifact.repository.id.clone(),
                    summary.artifact.number,
                ),
                summary.state.clone(),
            );
        }
    }

    attach_states(&mut bundle.primary, &states);
    for snapshot in &mut bundle.lineage {
        attach_states(snapshot, &states);
    }
}

fn attach_states(snapshot: &mut ArtifactSnapshot, states: &BTreeMap<(String, u64), String>) {
    let Some(workflow) = snapshot.workflow.as_mut() else {
        return;
    };
    for child in &mut workflow.children {
        child.state = states
            .get(&(child.repository_id.clone(), child.number))
            .cloned();
    }
}

/// Removes one optional child state from the deterministic end of a snapshot.
pub(super) fn drop_optional_child_state(snapshot: &mut ArtifactSnapshot) -> bool {
    let Some(workflow) = snapshot.workflow.as_mut() else {
        return false;
    };
    let Some(child) = workflow
        .children
        .iter_mut()
        .rev()
        .find(|child| child.state.is_some())
    else {
        return false;
    };
    child.state = None;
    true
}

/// Removes one optional child identity from the deterministic end of a snapshot.
pub(super) fn drop_optional_child(snapshot: &mut ArtifactSnapshot) -> bool {
    let Some(workflow) = snapshot.workflow.as_mut() else {
        return false;
    };
    if workflow.children.pop().is_none() {
        return false;
    }
    if workflow == &ArtifactWorkflowContext::default() {
        snapshot.workflow = None;
    }
    true
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use temper_forge::{ItemNumber, RepositoryId};
    use temper_protocol_context::{ArtifactRelationType, ArtifactSummary};
    use temper_workflow::{
        ArtifactKindId, CreateIssueIntentChild, CreateIssuesCompletion, CreateIssuesIntent,
        WorkflowMetadata, render_metadata_block,
    };

    use super::*;

    fn input(body: String, kind: Option<&str>) -> SnapshotInput {
        SnapshotInput {
            repository: ArtifactRepository {
                id: "forge:source".into(),
                path: "ai/source".into(),
            },
            artifact_type: ArtifactType::Issue,
            number: 7,
            title: "artifact".into(),
            body,
            labels: vec!["ready".into(), "code".into(), "ready".into()],
            state: "open".into(),
            workflow_kind: kind.map(str::to_string),
        }
    }

    fn child(
        repository_id: &str,
        number: Option<u64>,
        title: &str,
        body_hex: &str,
    ) -> CreateIssueIntentChild {
        CreateIssueIntentChild {
            slug: title.into(),
            title: title.into(),
            body_hex: body_hex.into(),
            final_labels: vec!["code".into()],
            dependencies: vec!["large-private-dependency".into()],
            repository_id: RepositoryId::new(repository_id),
            correlation_key: format!("correlation-{title}"),
            number: number.map(ItemNumber::new),
            wired: true,
            activated: true,
        }
    }

    fn intent(children: Vec<CreateIssueIntentChild>, payload: &str) -> CreateIssuesIntent {
        CreateIssuesIntent {
            transition: "fanout".into(),
            effect_index: 0,
            correlation_key: "intent".into(),
            record_parent_dependencies: true,
            children,
            completion: Some(CreateIssuesCompletion {
                body_hex: Some(payload.into()),
                ..Default::default()
            }),
            parent_wired: true,
            completed: true,
        }
    }

    #[test]
    fn projection_preserves_authored_bytes_and_normalizes_compact_context() {
        let mut intents = BTreeMap::new();
        intents.insert(
            "z".into(),
            intent(
                vec![
                    child("forge:other", Some(11), "z-title", "private-child-body"),
                    child("forge:other", None, "not-persisted", "private-draft-body"),
                ],
                "private-completion",
            ),
        );
        intents.insert(
            "a".into(),
            intent(
                vec![child(
                    "forge:other",
                    Some(11),
                    "a-title",
                    "different-private-child-body",
                )],
                "different-private-completion",
            ),
        );
        let metadata = WorkflowMetadata {
            kind: Some(ArtifactKindId::new("code")),
            parents: vec![
                ArtifactRef::same_repo(ItemNumber::new(9)),
                ArtifactRef::in_repo("forge:source", ItemNumber::new(9)),
                ArtifactRef::in_repo("forge:other", ItemNumber::new(2)),
            ],
            dependencies: vec![
                ArtifactRef::in_repo("forge:other", ItemNumber::new(8)),
                ArtifactRef::same_repo(ItemNumber::new(3)),
                ArtifactRef::same_repo(ItemNumber::new(3)),
            ],
            correlation_key: Some("  stable-correlation  ".into()),
            target_branch: Some("  main  ".into()),
            repaired_head: Some("private-repair-state".into()),
            staged: true,
            create_issue_intents: intents,
            ..Default::default()
        };
        let authored = "authored-prefix\n<!-- ordinary -->\nauthored-suffix\n";
        let raw = format!("{authored}{}", render_metadata_block(&metadata));

        let snapshot = project_snapshot(input(raw, Some("conflicting-fallback")));

        assert_eq!(snapshot.body, authored);
        assert_eq!(snapshot.labels, ["code", "ready"]);
        assert_eq!(snapshot.workflow_kind.as_deref(), Some("code"));
        let workflow = snapshot.workflow.as_ref().expect("workflow projection");
        assert_eq!(workflow.kind.as_deref(), Some("code"));
        assert_eq!(
            workflow.parents,
            [
                WorkflowArtifactReference {
                    repository_id: "forge:other".into(),
                    number: 2,
                },
                WorkflowArtifactReference {
                    repository_id: "forge:source".into(),
                    number: 9,
                },
            ]
        );
        assert_eq!(
            workflow.dependencies,
            [
                WorkflowArtifactReference {
                    repository_id: "forge:other".into(),
                    number: 8,
                },
                WorkflowArtifactReference {
                    repository_id: "forge:source".into(),
                    number: 3,
                },
            ]
        );
        assert_eq!(workflow.target_branch.as_deref(), Some("main"));
        assert_eq!(
            workflow.correlation_key.as_deref(),
            Some("stable-correlation")
        );
        assert_eq!(
            workflow.children,
            [WorkflowChildIdentity {
                repository_id: "forge:other".into(),
                number: 11,
                title: "a-title".into(),
                state: None,
            }]
        );
        let serialized = serde_json::to_string(&snapshot).unwrap();
        for forbidden in [
            "body_hex",
            "create_issue_intents",
            "private-child-body",
            "private-draft-body",
            "private-completion",
            "repaired_head",
            "staged",
            "wired",
            "activated",
        ] {
            assert!(!serialized.contains(forbidden), "leaked {forbidden}");
        }
    }

    #[test]
    fn bookkeeping_growth_and_new_identity_have_only_compact_effects() {
        let metadata = |payload: &str, second_child: bool| {
            let mut children = vec![child("forge:source", Some(12), "child", payload)];
            if second_child {
                children.push(child("forge:other", Some(13), "second child", payload));
            }
            let mut intents = BTreeMap::new();
            intents.insert("intent".into(), intent(children, payload));
            WorkflowMetadata {
                kind: Some(ArtifactKindId::new("plan")),
                repaired_head: Some(payload.into()),
                create_issue_intents: intents,
                ..Default::default()
            }
        };
        let raw = |payload: &str, second_child| {
            format!(
                "authored\n{}",
                render_metadata_block(&metadata(payload, second_child))
            )
        };

        let compact = project_snapshot(input(raw("x", false), None));
        assert_eq!(
            compact,
            project_snapshot(input(raw(&"x".repeat(100_000), false), None))
        );

        let expanded = project_snapshot(input(raw("x", true), None));
        assert_eq!(expanded.body, compact.body);
        assert_eq!(
            expanded
                .workflow
                .as_ref()
                .unwrap()
                .children
                .iter()
                .find(|child| child.number == 13)
                .unwrap(),
            &WorkflowChildIdentity {
                repository_id: "forge:other".into(),
                number: 13,
                title: "second child".into(),
                state: None,
            }
        );
        let mut without_new_identity = expanded;
        without_new_identity
            .workflow
            .as_mut()
            .unwrap()
            .children
            .retain(|child| child.number != 13);
        assert_eq!(without_new_identity, compact);
    }

    #[test]
    fn available_child_state_joins_from_bounded_summaries_only() {
        let metadata = WorkflowMetadata {
            kind: Some(ArtifactKindId::new("plan")),
            create_issue_intents: BTreeMap::from([(
                "intent".into(),
                intent(
                    vec![
                        child("forge:other", Some(12), "available", "private"),
                        child("forge:missing", Some(13), "missing", "private"),
                    ],
                    "private",
                ),
            )]),
            ..Default::default()
        };
        let raw = format!("authored\n{}", render_metadata_block(&metadata));
        let mut bundle = ArtifactContextBundle::new(project_snapshot(input(raw, None)));
        bundle.validation_scope.push(ArtifactSummary {
            artifact: ArtifactReference {
                repository: ArtifactRepository {
                    id: "forge:other".into(),
                    path: "ai/other".into(),
                },
                artifact_type: ArtifactType::Issue,
                number: 12,
            },
            title: "available".into(),
            labels: Vec::new(),
            state: "closed".into(),
            workflow_kind: Some("code".into()),
            relation_type: ArtifactRelationType::Dependency,
            source: bundle.primary.artifact.clone(),
        });

        attach_available_child_states(&mut bundle);

        let children = &bundle.primary.workflow.as_ref().unwrap().children;
        assert_eq!(
            children
                .iter()
                .find(|child| child.number == 12)
                .unwrap()
                .state
                .as_deref(),
            Some("closed")
        );
        assert!(
            children
                .iter()
                .find(|child| child.number == 13)
                .unwrap()
                .state
                .is_none()
        );
    }

    #[test]
    fn missing_and_malformed_metadata_remain_visible() {
        let plain = "plain authored body".to_string();
        let snapshot = project_snapshot(input(plain.clone(), Some("code")));
        assert_eq!(snapshot.body, plain);
        assert_eq!(snapshot.workflow_kind.as_deref(), Some("code"));
        assert_eq!(
            snapshot.workflow.and_then(|workflow| workflow.kind),
            Some("code".into())
        );

        let malformed = format!("{}\n{{broken}}\n-->", temper_workflow::METADATA_BEGIN);
        let snapshot = project_snapshot(input(malformed.clone(), None));
        assert_eq!(snapshot.body, malformed);
        assert!(snapshot.workflow_kind.is_none());
        assert!(snapshot.workflow.is_none());

        let unterminated = format!("{}\n{{}}", temper_workflow::METADATA_BEGIN);
        let snapshot = project_snapshot(input(unterminated.clone(), None));
        assert_eq!(snapshot.body, unterminated);
    }
}
