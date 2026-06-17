//! Confirmation tests for the reference delivery workflow fixture.

use chrono::{DateTime, Duration, Utc};
use temper_forge::{
    BranchRef, Issue, IssueState, ItemNumber, PullRequest, PullRequestState, ReviewDecision,
};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, CiStatus, ClassifiedArtifact, ClassifiedRelation, Classifier,
    DependencyStatus, Effect, GateCondition, GateId, GateSignals, IntakeAuthor, LabelId,
    PlanDiagnostic, QueueId, RawWorkflowSpec, RelationKind, ReviewStatus, RoleId, TransitionId,
    ValidatedWorkflow, VerdictId, WorkflowEffect, WorkflowMetadata, compile, render_metadata_block,
};

#[path = "reference_delivery/classification.rs"]
mod classification;
#[path = "reference_delivery/dependencies.rs"]
mod dependencies;
#[path = "reference_delivery/fixture.rs"]
mod fixture;
#[path = "reference_delivery/gates.rs"]
mod gates;
#[path = "reference_delivery/intake_review.rs"]
mod intake_review;
#[path = "reference_delivery/planning.rs"]
mod planning;

const FIXTURE: &str = include_str!("../fixtures/reference-delivery.json");

fn ts() -> DateTime<Utc> {
    "2026-05-29T00:00:00Z".parse().expect("valid timestamp")
}

fn fixture_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(FIXTURE).expect("fixture is valid JSON for RawWorkflowSpec");
    spec.validate()
        .expect("reference delivery fixture validates")
}

fn issue(number: u64, labels: &[&str]) -> Issue {
    issue_with_dependencies(number, labels, &[])
}

fn issue_with_dependencies(number: u64, labels: &[&str], dependencies: &[u64]) -> Issue {
    Issue {
        id: "issue-1".into(),
        repo_id: "repo-1".into(),
        number: ItemNumber::new(number),
        title: "title".to_string(),
        body: String::new(),
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

fn pull_request(number: u64, labels: &[&str]) -> PullRequest {
    PullRequest {
        id: "pr-1".into(),
        repo_id: "repo-1".into(),
        number: ItemNumber::new(number),
        title: "title".to_string(),
        body: String::new(),
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

fn classify_issue(
    workflow: &ValidatedWorkflow,
    number: u64,
    labels: &[&str],
) -> ClassifiedArtifact {
    Classifier::new(workflow)
        .classify_issue(&issue(number, labels))
        .expect("issue classifies")
}

fn classify_pr(workflow: &ValidatedWorkflow, number: u64, labels: &[&str]) -> ClassifiedArtifact {
    Classifier::new(workflow)
        .classify_pull_request(&pull_request(number, labels))
        .expect("pull request classifies")
}

fn classify_pr_updated_at(
    workflow: &ValidatedWorkflow,
    number: u64,
    labels: &[&str],
    updated_at: DateTime<Utc>,
) -> ClassifiedArtifact {
    let mut pull_request = pull_request(number, labels);
    pull_request.updated_at = updated_at;
    Classifier::new(workflow)
        .classify_pull_request(&pull_request)
        .expect("pull request classifies")
}
