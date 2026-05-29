//! Confirmation tests for the reference delivery workflow fixture.
//!
//! These prove the label-state-machine core of the reference delivery design
//! (see `docs/explanation/reference-workflow.md`) validates, compiles, and
//! plans through the current `harness-workflow` primitives. Remaining execution
//! and modeling gaps are recorded in `docs/explanation/reference-workflow-gaps.md`,
//! not here.

use chrono::{DateTime, Duration, Utc};
use harness_forge::{BranchRef, Issue, IssueState, ItemNumber, PullRequest, PullRequestState};
use harness_workflow::{
    compile, render_metadata_block, ArtifactKindId, ClassifiedArtifact, ClassifiedRelation,
    Classifier, GateCondition, GateId, LabelId, LabelUsage, PlanDiagnostic, QueueId,
    RawWorkflowSpec, RelationKind, RoleId, StateDimensionId, StateId, TransitionId,
    ValidatedWorkflow, WorkflowEffect, WorkflowMetadata,
};

/// The checked-in reference delivery workflow fixture.
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
        merge: None,
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

#[test]
fn reference_fixture_validates_with_expected_shape() {
    let workflow = fixture_workflow();
    assert_eq!(workflow.name(), "reference-delivery");
    assert_eq!(workflow.roles().len(), 5);
    assert_eq!(workflow.artifact_kinds().len(), 5);
    assert_eq!(workflow.state_dimensions().len(), 9);
    assert_eq!(workflow.queues().len(), 10);
    assert_eq!(workflow.transitions().len(), 19);
    assert_eq!(workflow.gates().len(), 3);
    assert_eq!(workflow.relations().len(), 5);
    assert!(workflow
        .transitions()
        .iter()
        .all(|transition| !transition.id.as_str().starts_with("record_ci_")));

    let ci_gate = workflow
        .gates()
        .iter()
        .find(|gate| gate.id.as_str() == "ci_gate")
        .expect("ci_gate is declared");
    assert_eq!(
        ci_gate.condition.as_ref(),
        Some(&GateCondition::StateEquals {
            dimension: StateDimensionId::new("ci"),
            state: StateId::new("passed"),
        })
    );

    let owner_alignment = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "owner_alignment")
        .expect("owner_alignment queue is declared");
    assert_eq!(owner_alignment.min_depth, Some(5));
    assert_eq!(owner_alignment.max_age, Some(Duration::days(7)));
}

#[test]
fn reference_fixture_compiles_every_role() {
    let compiled = compile(&fixture_workflow());
    let mut ids: Vec<String> = compiled.roles().iter().map(|r| r.id.to_string()).collect();
    ids.sort();
    assert_eq!(
        ids,
        vec!["architect", "engineer", "owner", "reviewer", "tester"]
    );

    let ci_passed = compiled
        .labels()
        .get(&LabelId::new("ci-passed"))
        .expect("ci-passed label is in the manifest");
    assert!(ci_passed.usages.iter().any(|usage| matches!(
        usage,
        LabelUsage::GateCondition { gate } if gate.as_str() == "ci_gate"
    )));

    let owner_alignment = compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "owner_alignment")
        .expect("owner_alignment queue is compiled");
    assert_eq!(owner_alignment.min_depth, Some(5));
    assert_eq!(owner_alignment.max_age, Some(Duration::days(7)));
}

#[test]
fn intake_triage_is_a_normal_queue_match() {
    // Human-filed issues enter as `untriaged` and the architect triages them.
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let intake = classify_issue(&workflow, 1, &["untriaged"]);

    assert!(planner
        .matching_queues(&intake)
        .contains(&harness_workflow::QueueId::new("design_triage")));

    let plan = planner
        .plan_transition(
            &TransitionId::new("triage_to_code"),
            &RoleId::new("architect"),
            &intake,
        )
        .expect("architect can triage an untriaged issue into code");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::RemoveLabel(LabelId::new("untriaged")),
            WorkflowEffect::AddLabel(LabelId::new("code")),
            WorkflowEffect::AddLabel(LabelId::new("code-ready")),
        ]
    );
}

#[test]
fn owner_alignment_queue_activates_by_depth_or_age() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let queue = QueueId::new("owner_alignment");
    let now = ts();
    let fresh = now - Duration::hours(1);
    let old = now - Duration::days(8);

    let under_depth: Vec<ClassifiedArtifact> = (1..=4)
        .map(|number| {
            classify_pr_updated_at(
                &workflow,
                number,
                &["implementation", "owner-pending"],
                fresh,
            )
        })
        .collect();
    assert!(planner.matching_queues(&under_depth[0]).contains(&queue));
    assert!(!planner.queue_active(&queue, &under_depth, now));

    let at_depth: Vec<ClassifiedArtifact> = (1..=5)
        .map(|number| {
            classify_pr_updated_at(
                &workflow,
                number,
                &["implementation", "owner-pending"],
                fresh,
            )
        })
        .collect();
    assert!(planner.queue_active(&queue, &at_depth, now));

    let old_enough = vec![classify_pr_updated_at(
        &workflow,
        42,
        &["implementation", "owner-pending"],
        old,
    )];
    assert!(planner.queue_active(&queue, &old_enough, now));

    let empty: Vec<ClassifiedArtifact> = Vec::new();
    assert!(!planner.queue_active(&queue, &empty, now));
}

#[test]
fn engineer_claims_ready_code_but_reviewer_cannot() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let artifact = classify_issue(&workflow, 42, &["code", "code-ready"]);

    let plan = planner
        .plan_transition(
            &TransitionId::new("claim_code"),
            &RoleId::new("engineer"),
            &artifact,
        )
        .expect("engineer is authorized to claim ready code");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::RemoveLabel(LabelId::new("code-ready")),
            WorkflowEffect::AddLabel(LabelId::new("in-progress")),
            WorkflowEffect::SetAssignee {
                role: RoleId::new("engineer"),
            },
        ]
    );

    let error = planner
        .plan_transition(
            &TransitionId::new("claim_code"),
            &RoleId::new("reviewer"),
            &artifact,
        )
        .expect_err("reviewer must not claim code issues");
    assert!(error.diagnostics().contains(&PlanDiagnostic::Unauthorized {
        transition: TransitionId::new("claim_code"),
        role: RoleId::new("reviewer"),
    }));
}

#[test]
fn reference_metadata_relations_classify_to_declared_kinds() {
    let workflow = fixture_workflow();
    let classifier = Classifier::new(&workflow);
    let code_body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        parents: vec![ItemNumber::new(2)],
        dependencies: vec![ItemNumber::new(3)],
        ..WorkflowMetadata::default()
    });
    let code = classifier
        .classify_issue(&Issue {
            body: code_body,
            ..issue(1, &["code", "blocked-on-dependency"])
        })
        .expect("code issue with relations classifies");

    assert_eq!(
        code.relations,
        vec![
            ClassifiedRelation {
                kind: RelationKind::Parent,
                source: ArtifactKindId::new("code"),
                target: ItemNumber::new(2),
                target_kinds: vec![ArtifactKindId::new("design"), ArtifactKindId::new("epic")],
            },
            ClassifiedRelation {
                kind: RelationKind::Dependency,
                source: ArtifactKindId::new("code"),
                target: ItemNumber::new(3),
                target_kinds: vec![ArtifactKindId::new("code")],
            },
        ]
    );

    let pr_body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ItemNumber::new(1)],
        ..WorkflowMetadata::default()
    });
    let pr = classifier
        .classify_pull_request(&PullRequest {
            body: pr_body,
            ..pull_request(4, &["implementation"])
        })
        .expect("implementation PR relation classifies");
    assert_eq!(
        pr.relations,
        vec![ClassifiedRelation {
            kind: RelationKind::ProducedPr,
            source: ArtifactKindId::new("implementation_pr"),
            target: ItemNumber::new(1),
            target_kinds: vec![ArtifactKindId::new("code")],
        }]
    );
}

#[test]
fn engineer_open_pr_expresses_pr_creation() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let artifact = classify_issue(&workflow, 43, &["code", "in-progress"]);

    let plan = planner
        .plan_transition(
            &TransitionId::new("open_pr"),
            &RoleId::new("engineer"),
            &artifact,
        )
        .expect("engineer can request PR creation from in-progress code");
    assert_eq!(
        plan.effects,
        vec![WorkflowEffect::CreatePullRequest {
            correlation_key: None,
        }]
    );
    assert!(plan.postconditions.is_empty());
}

#[test]
fn three_gate_merge_requires_review_testing_and_ci() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    // All three gates satisfied: review approved, testing passed, CI passed.
    let ready = classify_pr(
        &workflow,
        10,
        &[
            "implementation",
            "review-approved",
            "testing-passed",
            "ci-passed",
        ],
    );
    let plan = planner
        .plan_transition(
            &TransitionId::new("approve_merge"),
            &RoleId::new("owner"),
            &ready,
        )
        .expect("owner can approve a fully gated merge");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::AddLabel(LabelId::new("merge-ready")),
            WorkflowEffect::MergePullRequest,
        ]
    );

    // CI gate unsatisfied: the merge cannot be planned.
    let pending = classify_pr(
        &workflow,
        11,
        &["implementation", "review-approved", "testing-passed"],
    );
    let error = planner
        .plan_transition(
            &TransitionId::new("approve_merge"),
            &RoleId::new("owner"),
            &pending,
        )
        .expect_err("an ungated merge must not plan");
    assert!(error
        .diagnostics()
        .contains(&PlanDiagnostic::GateNotSatisfied {
            transition: TransitionId::new("approve_merge"),
            gate: GateId::new("ci_gate"),
        }));
}

#[test]
fn failed_gates_route_back_to_engineer_queues() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    let changes = classify_pr(
        &workflow,
        20,
        &["implementation", "review-changes-requested"],
    );
    assert!(planner
        .matching_queues(&changes)
        .contains(&harness_workflow::QueueId::new("pr_changes_requested")));

    let failed = classify_pr(&workflow, 21, &["implementation", "testing-failed"]);
    assert!(planner
        .matching_queues(&failed)
        .contains(&harness_workflow::QueueId::new("pr_testing_failed")));
}
