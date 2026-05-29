//! Tests for Forge artifact classification (Phase 3).

use chrono::{DateTime, Utc};
use harness_forge::{BranchRef, Issue, IssueState, ItemNumber, PullRequest, PullRequestState};
use harness_workflow::{
    render_metadata_block, ArtifactKindId, ArtifactSource, ArtifactTarget,
    ClassificationDiagnostic, Classifier, LabelId, RawWorkflowSpec, StateDimensionId, StateId,
    ValidatedWorkflow, WorkflowMetadata,
};

fn ts() -> DateTime<Utc> {
    "2026-05-29T00:00:00Z".parse().expect("valid timestamp")
}

/// A two-artifact workflow: a `code` issue and an `implementation_pr` pull
/// request, each with an identifying label and exclusive state dimensions.
fn workflow() -> ValidatedWorkflow {
    let json = r#"{
        "name": "five-role",
        "labels": [
            {"id": "code"}, {"id": "implementation"},
            {"id": "ready"}, {"id": "in-progress"}, {"id": "blocked"},
            {"id": "needs-review"}, {"id": "review-approved"}
        ],
        "artifact_kinds": [
            {"id": "code", "target": "issue", "identifying_labels": ["code"]},
            {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
        ],
        "state_dimensions": [
            {"id": "code_lifecycle", "exclusive": true, "states": [
                {"id": "ready", "label": "ready"},
                {"id": "in_progress", "label": "in-progress"},
                {"id": "blocked", "label": "blocked"}
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
        merge: None,
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
    let body = "<!-- harness:workflow\n{ broken json\n-->";

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
