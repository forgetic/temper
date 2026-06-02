//! Tests for Forge artifact classification (Phase 3).

use chrono::{DateTime, Utc};
use temper_forge::{BranchRef, Issue, IssueState, ItemNumber, PullRequest, PullRequestState};
use temper_workflow::{
    render_metadata_block, ArtifactKindId, ArtifactRef, ArtifactSource, ArtifactTarget,
    ClassificationDiagnostic, ClassifiedRelation, Classifier, LabelId, RawWorkflowSpec,
    RelationKind, StateDimensionId, StateId, ValidatedWorkflow, WorkflowMetadata,
};

fn ts() -> DateTime<Utc> {
    "2026-05-29T00:00:00Z".parse().expect("valid timestamp")
}

/// A small workflow with an `epic` issue, a `code` issue, and an
/// `implementation_pr` pull request, plus relation declarations.
fn workflow() -> ValidatedWorkflow {
    let json = r#"{
        "name": "five-role",
        "labels": [
            {"id": "epic"}, {"id": "code"}, {"id": "implementation"},
            {"id": "ready"}, {"id": "in-progress"}, {"id": "blocked"},
            {"id": "needs-review"}, {"id": "review-approved"}
        ],
        "artifact_kinds": [
            {"id": "epic", "target": "issue", "identifying_labels": ["epic"]},
            {"id": "code", "target": "issue", "identifying_labels": ["code"]},
            {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
        ],
        "relations": [
            {"kind": "parent", "source": "code", "target": "epic"},
            {"kind": "dependency", "source": "code", "target": "code"},
            {"kind": "produced_pr", "source": "implementation_pr", "target": "code"}
        ],
        "state_dimensions": [
            {"id": "code_lifecycle", "exclusive": true, "states": [
                {"id": "ready", "label": "ready", "artifacts": ["code"]},
                {"id": "in_progress", "label": "in-progress", "artifacts": ["code"]},
                {"id": "blocked", "label": "blocked", "artifacts": ["code"]}
            ]},
            {"id": "review", "exclusive": true, "states": [
                {"id": "needs_review", "label": "needs-review"},
                {"id": "approved", "label": "review-approved"}
            ]}
        ]
    }"#;
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("fixture parses");
    spec.validate().expect("fixture validates")
}

fn issue(number: u64, labels: &[&str], body: &str) -> Issue {
    issue_with_dependencies(number, labels, body, &[])
}

fn issue_with_dependencies(
    number: u64,
    labels: &[&str],
    body: &str,
    dependencies: &[u64],
) -> Issue {
    Issue {
        id: "issue-1".into(),
        repo_id: "repo-1".into(),
        number: ItemNumber::new(number),
        title: "title".to_string(),
        body: body.to_string(),
        state: IssueState::Open,
        author_id: "user-1".into(),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        assignees: Vec::new(),
        dependencies: dependencies.iter().copied().map(ItemNumber::new).collect(),
        version: Default::default(),
        created_at: ts(),
        updated_at: ts(),
        closed_at: None,
    }
}

fn pull_request(number: u64, labels: &[&str], body: &str) -> PullRequest {
    PullRequest {
        id: "pr-1".into(),
        repo_id: "repo-1".into(),
        number: ItemNumber::new(number),
        title: "title".to_string(),
        body: body.to_string(),
        state: PullRequestState::Open,
        author_id: "user-1".into(),
        source: BranchRef {
            repository_id: "repo-1".into(),
            branch: "feature".to_string(),
        },
        target: BranchRef {
            repository_id: "repo-1".into(),
            branch: "main".to_string(),
        },
        head_sha: None,
        base_sha: None,
        labels: labels.iter().map(|s| s.to_string()).collect(),
        assignees: Vec::new(),
        requested_reviewers: Vec::new(),
        dependencies: Vec::new(),
        merge: None,
        version: Default::default(),
        created_at: ts(),
        updated_at: ts(),
        closed_at: None,
    }
}

#[test]
fn issue_is_classified_as_code_from_labels() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);

    let artifact = classifier
        .classify_issue(&issue(42, &["code", "ready"], ""))
        .expect("code issue classifies");

    assert_eq!(artifact.kind, ArtifactKindId::new("code"));
    assert_eq!(artifact.target, ArtifactTarget::Issue);
    assert_eq!(
        artifact.source,
        ArtifactSource::Issue {
            number: ItemNumber::new(42)
        }
    );
    assert_eq!(
        artifact
            .states
            .get(&StateDimensionId::new("code_lifecycle")),
        Some(&vec![StateId::new("ready")])
    );
    assert!(artifact.metadata.is_empty());
}

#[test]
fn pull_request_is_classified_as_implementation_pr_from_labels() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);

    let artifact = classifier
        .classify_pull_request(&pull_request(7, &["implementation", "needs-review"], ""))
        .expect("implementation PR classifies");

    assert_eq!(artifact.kind, ArtifactKindId::new("implementation_pr"));
    assert_eq!(artifact.target, ArtifactTarget::PullRequest);
    assert_eq!(
        artifact.source,
        ArtifactSource::PullRequest {
            number: ItemNumber::new(7)
        }
    );
    assert_eq!(
        artifact.states.get(&StateDimensionId::new("review")),
        Some(&vec![StateId::new("needs_review")])
    );
}

#[test]
fn exclusive_state_conflict_is_diagnosed() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);

    let error = classifier
        .classify_issue(&issue(1, &["code", "ready", "in-progress"], ""))
        .expect_err("conflicting lifecycle labels must fail");

    assert!(error.diagnostics().iter().any(|d| matches!(
        d,
        ClassificationDiagnostic::ExclusiveStateConflict { dimension, states }
            if dimension == &StateDimensionId::new("code_lifecycle") && states.len() == 2
    )));
}

#[test]
fn state_limited_to_another_artifact_kind_is_diagnosed() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);

    let error = classifier
        .classify_issue(&issue(1, &["epic", "ready"], ""))
        .expect_err("ready is only legal for code in this fixture");

    assert!(error
        .diagnostics()
        .contains(&ClassificationDiagnostic::StateNotAllowedForArtifact {
            artifact: ArtifactKindId::new("epic"),
            dimension: StateDimensionId::new("code_lifecycle"),
            state: StateId::new("ready"),
        }));
}

#[test]
fn unmatched_issue_is_unclassified() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);

    let error = classifier
        .classify_issue(&issue(1, &["ready"], ""))
        .expect_err("no identifying label must fail");

    assert!(error
        .diagnostics()
        .contains(&ClassificationDiagnostic::Unclassified {
            target: ArtifactTarget::Issue,
        }));
}

#[test]
fn missing_metadata_block_still_classifies_by_labels() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);

    let artifact = classifier
        .classify_issue(&issue(1, &["code"], "free-form body with no metadata"))
        .expect("classifies without metadata");

    assert_eq!(artifact.kind, ArtifactKindId::new("code"));
    assert!(artifact.metadata.is_empty());
    assert!(!artifact
        .states
        .contains_key(&StateDimensionId::new("code_lifecycle")));
}

#[test]
fn malformed_metadata_is_reported_deterministically() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);
    let body = "<!-- temper:workflow\n{ broken json\n-->";

    let error = classifier
        .classify_issue(&issue(1, &["code"], body))
        .expect_err("malformed metadata must fail");

    assert!(error
        .diagnostics()
        .iter()
        .any(|d| matches!(d, ClassificationDiagnostic::MalformedMetadata { .. })));
}

#[test]
fn metadata_kind_drift_reports_missing_identifying_label() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);
    let body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        ..WorkflowMetadata::default()
    });

    // Metadata claims `code` but the identifying `code` label is absent.
    let error = classifier
        .classify_issue(&issue(1, &["ready"], &body))
        .expect_err("label drift must fail");

    assert!(error
        .diagnostics()
        .contains(&ClassificationDiagnostic::MissingIdentifyingLabel {
            kind: ArtifactKindId::new("code"),
            label: LabelId::new("code"),
        }));
}

#[test]
fn metadata_parents_and_dependencies_surface_typed_relations() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);
    let body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        parents: vec![ArtifactRef::same_repo(ItemNumber::new(12))],
        dependencies: vec![ArtifactRef::same_repo(ItemNumber::new(34))],
        ..WorkflowMetadata::default()
    });

    let artifact = classifier
        .classify_issue(&issue(1, &["code"], &body))
        .expect("code issue with relations classifies");

    assert_eq!(
        artifact.relations,
        vec![
            ClassifiedRelation {
                kind: RelationKind::Parent,
                source: ArtifactKindId::new("code"),
                target: ArtifactRef::same_repo(ItemNumber::new(12)),
                target_kinds: vec![ArtifactKindId::new("epic")],
            },
            ClassifiedRelation {
                kind: RelationKind::Dependency,
                source: ArtifactKindId::new("code"),
                target: ArtifactRef::same_repo(ItemNumber::new(34)),
                target_kinds: vec![ArtifactKindId::new("code")],
            },
        ]
    );
}

#[test]
fn repo_qualified_metadata_relation_classifies_with_explicit_target_repo() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);
    let cross_repo = ArtifactRef::in_repo("repo-other", ItemNumber::new(34));
    let body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        dependencies: vec![cross_repo.clone()],
        ..WorkflowMetadata::default()
    });

    let artifact = classifier
        .classify_issue(&issue(1, &["code"], &body))
        .expect("code issue with cross-repo relation classifies");

    assert_eq!(
        artifact.relations,
        vec![ClassifiedRelation {
            kind: RelationKind::Dependency,
            source: ArtifactKindId::new("code"),
            target: cross_repo,
            target_kinds: vec![ArtifactKindId::new("code")],
        }]
    );
}

#[test]
fn native_dependencies_override_same_repo_metadata_but_keep_cross_repo_fallback() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);
    let cross_repo = ArtifactRef::in_repo("repo-other", ItemNumber::new(78));
    let body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        dependencies: vec![
            ArtifactRef::same_repo(ItemNumber::new(34)),
            cross_repo.clone(),
        ],
        ..WorkflowMetadata::default()
    });

    let artifact = classifier
        .classify_issue(&issue_with_dependencies(1, &["code"], &body, &[56]))
        .expect("code issue with native dependency classifies");

    assert_eq!(
        artifact
            .relations
            .iter()
            .filter(|relation| relation.kind == RelationKind::Dependency)
            .collect::<Vec<_>>(),
        vec![
            &ClassifiedRelation {
                kind: RelationKind::Dependency,
                source: ArtifactKindId::new("code"),
                target: ArtifactRef::same_repo(ItemNumber::new(56)),
                target_kinds: vec![ArtifactKindId::new("code")],
            },
            &ClassifiedRelation {
                kind: RelationKind::Dependency,
                source: ArtifactKindId::new("code"),
                target: cross_repo,
                target_kinds: vec![ArtifactKindId::new("code")],
            },
        ]
    );
}

#[test]
fn metadata_parent_projection_can_surface_produced_pr_relation() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);
    let body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(ItemNumber::new(42))],
        ..WorkflowMetadata::default()
    });

    let artifact = classifier
        .classify_pull_request(&pull_request(7, &["implementation"], &body))
        .expect("PR with produced relation classifies");

    assert_eq!(
        artifact.relations,
        vec![ClassifiedRelation {
            kind: RelationKind::ProducedPr,
            source: ArtifactKindId::new("implementation_pr"),
            target: ArtifactRef::same_repo(ItemNumber::new(42)),
            target_kinds: vec![ArtifactKindId::new("code")],
        }]
    );
}

#[test]
fn metadata_target_mismatch_is_diagnosed() {
    let workflow = workflow();
    let classifier = Classifier::new(&workflow);
    let body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        ..WorkflowMetadata::default()
    });

    // `code` maps to an issue, but here it is asserted on a pull request.
    let error = classifier
        .classify_pull_request(&pull_request(1, &["code"], &body))
        .expect_err("target mismatch must fail");

    assert!(error.diagnostics().iter().any(|d| matches!(
        d,
        ClassificationDiagnostic::TargetMismatch {
            expected: ArtifactTarget::Issue,
            actual: ArtifactTarget::PullRequest,
            ..
        }
    )));
}
