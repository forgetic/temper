//! Focused reference-delivery PR-create transition tests.

use chrono::{DateTime, Utc};
use harness_forge::{Issue, IssueState, ItemNumber};
use harness_workflow::{
    ArtifactKindId, Classifier, LabelId, RawWorkflowSpec, RoleId, TransitionId, ValidatedWorkflow,
    WorkflowEffect,
};

const FIXTURE: &str = include_str!("../fixtures/reference-delivery.json");

fn ts() -> DateTime<Utc> {
    "2026-05-29T00:00:00Z".parse().expect("valid timestamp")
}

fn fixture_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("fixture parses");
    spec.validate().expect("reference fixture validates")
}

fn issue(number: u64, labels: &[&str]) -> Issue {
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
        dependencies: Vec::new(),
        version: Default::default(),
        created_at: ts(),
        updated_at: ts(),
        closed_at: None,
    }
}

#[test]
fn engineer_open_pr_claims_ready_code_and_expresses_pr_creation() {
    let workflow = fixture_workflow();
    let artifact = Classifier::new(&workflow)
        .classify_issue(&issue(43, &["code", "ready"]))
        .expect("code issue classifies");

    let plan = workflow
        .planner()
        .plan_transition(
            &TransitionId::new("open_pr"),
            &RoleId::new("engineer"),
            &artifact,
        )
        .expect("engineer can claim ready code and request PR creation");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::RemoveLabel(LabelId::new("ready")),
            WorkflowEffect::AddLabel(LabelId::new("in-progress")),
            WorkflowEffect::SetAssignee {
                role: RoleId::new("engineer"),
            },
            WorkflowEffect::CreatePullRequest {
                correlation_key: None,
            },
        ]
    );
    assert_eq!(artifact.kind, ArtifactKindId::new("code"));
}
