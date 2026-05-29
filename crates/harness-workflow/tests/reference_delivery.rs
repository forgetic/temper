//! Confirmation tests for the reference delivery workflow fixture.

use chrono::{DateTime, Duration, Utc};
use harness_forge::{BranchRef, Issue, IssueState, ItemNumber, PullRequest, PullRequestState};
use harness_workflow::{
    compile, render_metadata_block, ArtifactKindId, CiStatus, ClassifiedArtifact,
    ClassifiedRelation, Classifier, DependencyStatus, GateCondition, GateId, GateSignals, LabelId,
    PlanDiagnostic, QueueId, RawWorkflowSpec, RelationKind, ReviewStatus, RoleId, TransitionId,
    ValidatedWorkflow, WorkflowEffect, WorkflowMetadata,
};

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

#[test]
fn reference_fixture_validates_with_expected_shape() {
    let workflow = fixture_workflow();
    assert_eq!(workflow.name(), "reference-delivery");
    assert_eq!(workflow.roles().len(), 5);
    assert_eq!(workflow.artifact_kinds().len(), 5);
    assert_eq!(workflow.state_dimensions().len(), 3);
    assert_eq!(workflow.queues().len(), 10);
    assert_eq!(workflow.transitions().len(), 20);
    assert_eq!(workflow.gates().len(), 3);
    assert_eq!(workflow.relations().len(), 5);
    assert!(workflow
        .transitions()
        .iter()
        .all(|transition| !transition.id.as_str().starts_with("record_ci_")));
    assert!(!workflow
        .labels()
        .iter()
        .any(|label| label.as_str() == "merge-ready"
            || label.as_str().starts_with("ci-")
            || label.as_str().starts_with("review-")));

    let ci_gate = workflow
        .gates()
        .iter()
        .find(|gate| gate.id.as_str() == "ci_gate")
        .expect("ci_gate is declared");
    assert_eq!(ci_gate.condition.as_ref(), Some(&GateCondition::CiPassed));

    let owner_alignment = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "owner_alignment")
        .expect("owner_alignment queue is declared");
    assert_eq!(owner_alignment.min_depth, Some(5));
    assert_eq!(owner_alignment.max_age, Some(Duration::days(7)));

    let return_queue = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "pr_changes_requested")
        .expect("work-return queue is declared");
    assert_eq!(
        return_queue.condition.as_ref(),
        Some(&GateCondition::ReviewChangesRequested)
    );
    let architect_queue = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "needs_architect")
        .expect("needs_architect queue is declared");
    assert!(architect_queue
        .artifacts
        .contains(&ArtifactKindId::new("code")));
    assert!(architect_queue
        .artifacts
        .contains(&ArtifactKindId::new("implementation_pr")));
}

#[test]
fn reference_fixture_compiles_every_role() {
    let compiled = compile(&fixture_workflow());
    let mut ids: Vec<String> = compiled.roles().iter().map(|r| r.id.to_string()).collect();
    ids.sort();
    assert_eq!(
        ids,
        vec!["architect", "engineer", "human", "owner", "reviewer"]
    );

    assert!(compiled.labels().get(&LabelId::new("ci-passed")).is_none());
    assert!(compiled
        .labels()
        .get(&LabelId::new("review-approved"))
        .is_none());
    assert!(compiled
        .labels()
        .get(&LabelId::new("merge-ready"))
        .is_none());

    let owner_alignment = compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "owner_alignment")
        .expect("owner_alignment queue is compiled");
    assert_eq!(owner_alignment.min_depth, Some(5));
    assert_eq!(owner_alignment.max_age, Some(Duration::days(7)));

    assert!(compiled
        .labels()
        .get(&LabelId::new("testing-passed"))
        .is_none());
    assert!(compiled
        .labels()
        .get(&LabelId::new("testing-failed"))
        .is_none());

    let ci_failed = compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "pr_ci_failed")
        .expect("CI failure queue is compiled");
    assert_eq!(ci_failed.condition.as_ref(), Some(&GateCondition::CiFailed));
}

#[test]
fn intake_triage_is_a_normal_queue_match() {
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
            WorkflowEffect::AddLabel(LabelId::new("ready")),
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
    let artifact = classify_issue(&workflow, 42, &["code", "ready"]);

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
            WorkflowEffect::RemoveLabel(LabelId::new("ready")),
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
            ..issue(1, &["code", "blocked"])
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

fn classify_blocked_code(
    workflow: &ValidatedWorkflow,
    number: u64,
    dependencies: &[u64],
) -> ClassifiedArtifact {
    Classifier::new(workflow)
        .classify_issue(&issue_with_dependencies(
            number,
            &["code", "blocked"],
            dependencies,
        ))
        .expect("blocked code issue classifies")
}

#[test]
fn dependency_gate_unblocks_only_when_prerequisites_land() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let blocked = classify_blocked_code(&workflow, 50, &[51]);

    assert!(planner
        .dependency_unblocks(&blocked, &DependencyStatus::default())
        .is_empty());
    let gated = planner
        .plan_transition(
            &TransitionId::new("mark_code_ready"),
            &RoleId::new("architect"),
            &blocked,
        )
        .expect_err("mark_code_ready is gated until dependencies land");
    assert!(gated
        .diagnostics()
        .contains(&PlanDiagnostic::GateNotSatisfied {
            transition: TransitionId::new("mark_code_ready"),
            gate: GateId::new("dependency_gate"),
        }));

    let landed = DependencyStatus::landed([ItemNumber::new(51)]);
    let unblocks = planner.dependency_unblocks(&blocked, &landed);
    assert_eq!(unblocks.len(), 1);
    assert_eq!(unblocks[0].transition, TransitionId::new("mark_code_ready"));
    assert_eq!(
        unblocks[0].effects,
        vec![
            WorkflowEffect::RemoveLabel(LabelId::new("blocked")),
            WorkflowEffect::AddLabel(LabelId::new("ready")),
        ]
    );

    let signals = GateSignals::new().with_dependencies(landed.clone());
    let plan = planner
        .plan_transition_with(
            &TransitionId::new("mark_code_ready"),
            &RoleId::new("architect"),
            &blocked,
            &signals,
        )
        .expect("architect can mark ready once dependencies land");
    assert_eq!(plan.effects, unblocks[0].effects);
}

#[test]
fn dependency_gate_requires_every_prerequisite() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let blocked = classify_blocked_code(&workflow, 60, &[61, 62]);

    let partial = DependencyStatus::landed([ItemNumber::new(61)]);
    assert!(
        planner.dependency_unblocks(&blocked, &partial).is_empty(),
        "every prerequisite must land before the unblock"
    );

    let both = DependencyStatus::landed([ItemNumber::new(61), ItemNumber::new(62)]);
    assert_eq!(planner.dependency_unblocks(&blocked, &both).len(), 1);
}

#[test]
fn merge_requires_review_and_native_ci() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    let ready = classify_pr(&workflow, 10, &["implementation"]);
    let review = GateSignals::new().with_review(ReviewStatus::new(true, false));
    let blocked = planner
        .plan_transition_with(
            &TransitionId::new("approve_merge"),
            &RoleId::new("owner"),
            &ready,
            &review,
        )
        .expect_err("a merge cannot plan until the CI signal reports passed");
    assert!(blocked
        .diagnostics()
        .contains(&PlanDiagnostic::GateNotSatisfied {
            transition: TransitionId::new("approve_merge"),
            gate: GateId::new("ci_gate"),
        }));

    let signals = GateSignals::new()
        .with_ci(CiStatus::passed())
        .with_review(ReviewStatus::new(true, false));
    let plan = planner
        .plan_transition_with(
            &TransitionId::new("approve_merge"),
            &RoleId::new("owner"),
            &ready,
            &signals,
        )
        .expect("owner can approve a fully gated merge once CI passes");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::MergePullRequest,
            WorkflowEffect::AddLabel(LabelId::new("landed")),
            WorkflowEffect::AddLabel(LabelId::new("owner-pending")),
        ]
    );
}

#[test]
fn failed_gates_route_back_to_engineer_queues() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    let changes = classify_pr(&workflow, 20, &["implementation"]);
    let review_signal = GateSignals::new().with_review(ReviewStatus::new(false, true));
    assert!(planner
        .matching_queues_with(&changes, &review_signal)
        .contains(&QueueId::new("pr_changes_requested")));

    let ci_signal = GateSignals::new().with_ci(CiStatus::failed());
    let failed = classify_pr(&workflow, 21, &["implementation"]);
    assert!(planner
        .matching_queues_with(&failed, &ci_signal)
        .contains(&QueueId::new("pr_ci_failed")));
}

#[test]
fn attention_queues_route_architect_owner_and_human_work() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let architect = RoleId::new("architect");
    let owner = RoleId::new("owner");
    let human = RoleId::new("human");
    let needs_architect = QueueId::new("needs_architect");
    let needs_owner_queue = QueueId::new("needs_owner");
    let needs_human_queue = QueueId::new("needs_human");
    let needs_owner = LabelId::new("needs-owner");
    let needs_human = LabelId::new("needs-human");

    let architect_issue = classify_issue(&workflow, 30, &["code", "needs-architect"]);
    let architect_pr = classify_pr(&workflow, 31, &["implementation", "needs-architect"]);
    assert!(planner
        .matching_queues(&architect_issue)
        .contains(&needs_architect));
    assert!(planner
        .matching_queues(&architect_pr)
        .contains(&needs_architect));

    let design = classify_issue(&workflow, 32, &["design", "draft"]);
    let request_owner = TransitionId::new("request_owner_input");
    let request = planner
        .plan_transition(&request_owner, &architect, &design)
        .unwrap();
    assert_eq!(
        request.effects,
        vec![WorkflowEffect::AddLabel(needs_owner.clone())]
    );

    let owner_design = classify_issue(&workflow, 33, &["design", "needs-owner"]);
    assert!(planner
        .matching_queues(&owner_design)
        .contains(&needs_owner_queue));
    let request_human = TransitionId::new("request_human_input");
    let handoff = planner
        .plan_transition(&request_human, &owner, &owner_design)
        .unwrap();
    assert_eq!(
        handoff.effects,
        vec![
            WorkflowEffect::RemoveLabel(needs_owner),
            WorkflowEffect::AddLabel(needs_human.clone()),
        ]
    );

    let human_design = classify_issue(&workflow, 34, &["design", "needs-human"]);
    assert!(planner
        .matching_queues(&human_design)
        .contains(&needs_human_queue));
    let clear_human = TransitionId::new("clear_human_flag");
    let clear = planner
        .plan_transition(&clear_human, &human, &human_design)
        .unwrap();
    assert_eq!(
        clear.effects,
        vec![WorkflowEffect::RemoveLabel(needs_human)]
    );
}
