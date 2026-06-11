//! Confirmation tests for the reference delivery workflow fixture.

use chrono::{DateTime, Duration, Utc};
use temper_forge::{
    BranchRef, Issue, IssueState, ItemNumber, PullRequest, PullRequestState, ReviewDecision,
};
use temper_workflow::{
    compile, render_metadata_block, ArtifactKindId, ArtifactRef, CiStatus, ClassifiedArtifact,
    ClassifiedRelation, Classifier, DependencyStatus, GateCondition, GateId, GateSignals,
    IntakeAuthor, LabelId, PlanDiagnostic, QueueId, RawWorkflowSpec, RelationKind, ReviewStatus,
    RoleId, TransitionId, ValidatedWorkflow, VerdictId, WorkflowEffect, WorkflowMetadata,
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
    // The reference workflow seeds intake as the `human` role, so the knob is
    // set explicitly and behavior is unchanged.
    assert_eq!(
        workflow.intake_author(),
        Some(&IntakeAuthor::Role("human".into()))
    );
    assert_eq!(workflow.roles().len(), 6);
    assert_eq!(workflow.artifact_kinds().len(), 5);
    assert_eq!(workflow.state_dimensions().len(), 3);
    assert_eq!(workflow.queues().len(), 13);
    assert_eq!(workflow.transitions().len(), 33);
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
            || label.as_str() == "needs-merge"
            || label.as_str().starts_with("ci-")
            || label.as_str().starts_with("review-")));
    assert!(workflow
        .labels()
        .iter()
        .any(|label| label.as_str() == "landing"));
    assert!(workflow
        .labels()
        .iter()
        .any(|label| label.as_str() == "merge-conflict"));

    let mechanical = workflow
        .roles()
        .iter()
        .find(|role| role.id.as_str() == "mechanical")
        .expect("mechanical automation authority is declared");
    assert!(mechanical.queues.is_empty());

    let implementation_pr = workflow
        .artifact_kinds()
        .iter()
        .find(|kind| kind.id.as_str() == "implementation_pr")
        .expect("implementation PR kind is declared");
    assert_eq!(
        implementation_pr.identifying_labels,
        vec![LabelId::new("implementation")]
    );
    assert_eq!(
        implementation_pr.initial_labels,
        vec![LabelId::new("needs-reviewer")]
    );

    let ci_gate = workflow
        .gates()
        .iter()
        .find(|gate| gate.id.as_str() == "ci_gate")
        .expect("ci_gate is declared");
    assert_eq!(ci_gate.condition.as_ref(), Some(&GateCondition::CiPassed));

    let landing = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "landing")
        .expect("landing queue is declared");
    assert_eq!(landing.condition.as_ref(), Some(&GateCondition::CiPassed));
    let automation = landing
        .automation
        .as_ref()
        .expect("landing queue is mechanically serviced");
    assert_eq!(automation.actor, RoleId::new("mechanical"));
    assert_eq!(automation.transition, TransitionId::new("land_pr"));
    assert_eq!(
        automation.merge_conflict(),
        Some(&TransitionId::new("route_merge_conflict"))
    );
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

    // `intake` is the default (catch-all) issue kind: it declares no identifying
    // labels, so raw human intake (an issue with no labels) is admitted as a
    // normal work item rather than left unclassified.
    let intake = workflow
        .artifact_kinds()
        .iter()
        .find(|kind| kind.id.as_str() == "intake")
        .expect("intake kind is declared");
    assert!(
        intake.identifying_labels.is_empty(),
        "intake is the default issue kind and carries no identifying labels"
    );

    // `mark_untriaged` is the mechanical transition that stamps freshly filed
    // intake `untriaged` so the architect's `design_triage` queue can pick it up.
    let mark_untriaged = workflow
        .transitions()
        .iter()
        .find(|transition| transition.id.as_str() == "mark_untriaged")
        .expect("mark_untriaged transition is declared");
    assert_eq!(mark_untriaged.artifact, ArtifactKindId::new("intake"));
    assert!(mark_untriaged.roles.contains(&RoleId::new("mechanical")));

    // The `raw_intake` queue is what drives `mark_untriaged` from the live
    // mechanical scan: it selects the default-kind intake with no label filter
    // and runs the mechanical stamp. Without it, freshly filed unlabeled intake
    // never receives `untriaged` and the architect's `design_triage` queue never
    // matches, stalling the whole pipeline.
    let raw_intake = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "raw_intake")
        .expect("raw_intake mechanical queue is declared");
    assert!(raw_intake.labels.is_empty());
    assert!(raw_intake
        .artifacts
        .contains(&ArtifactKindId::new("intake")));
    let raw_intake_automation = raw_intake
        .automation
        .as_ref()
        .expect("raw_intake queue is mechanically serviced");
    assert_eq!(raw_intake_automation.actor, RoleId::new("mechanical"));
    assert_eq!(
        raw_intake_automation.transition,
        TransitionId::new("mark_untriaged")
    );
}

#[test]
fn reference_fixture_compiles_every_role() {
    let compiled = compile(&fixture_workflow());
    let mut ids: Vec<String> = compiled.roles().iter().map(|r| r.id.to_string()).collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            "architect",
            "engineer",
            "human",
            "mechanical",
            "owner",
            "reviewer"
        ]
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
    assert!(compiled
        .labels()
        .get(&LabelId::new("needs-merge"))
        .is_none());
    assert!(compiled.labels().get(&LabelId::new("landing")).is_some());
    assert!(compiled
        .labels()
        .get(&LabelId::new("merge-conflict"))
        .is_some());

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

    let landing = compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "landing")
        .expect("landing queue is compiled");
    let automation = landing
        .automation
        .as_ref()
        .expect("landing automation is compiled");
    assert_eq!(automation.actor, RoleId::new("mechanical"));
    assert_eq!(automation.transition, TransitionId::new("land_pr"));
    assert_eq!(
        automation.merge_conflict(),
        Some(&TransitionId::new("route_merge_conflict"))
    );

    let open_pr = compiled
        .roles()
        .iter()
        .find(|role| role.id.as_str() == "engineer")
        .expect("engineer role is compiled")
        .tools
        .iter()
        .find(|tool| tool.name == "open_pr")
        .expect("engineer has the open_pr tool");
    assert_eq!(
        open_pr.outcomes.get(&VerdictId::new("needs_architect")),
        Some(&TransitionId::new("request_code_architect_input")),
        "open_pr routes the needs_architect verdict to the code-artifact escalation transition"
    );
    let escalation = compiled
        .transitions()
        .iter()
        .find(|transition| transition.id.as_str() == "request_code_architect_input")
        .expect("escalation transition is compiled");
    assert_eq!(
        escalation.artifact,
        ArtifactKindId::new("code"),
        "escalation transition is legal on the open_pr artifact (code)"
    );
}

#[test]
fn intake_triage_is_a_normal_queue_match() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let intake = classify_issue(&workflow, 1, &["untriaged"]);

    assert!(planner
        .matching_queues(&intake)
        .contains(&temper_workflow::QueueId::new("design_triage")));

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
            classify_pr_updated_at(&workflow, number, &["implementation", "alignment"], fresh)
        })
        .collect();
    assert!(planner.matching_queues(&under_depth[0]).contains(&queue));
    assert!(!planner.queue_active(&queue, &under_depth, now));

    let at_depth: Vec<ClassifiedArtifact> = (1..=5)
        .map(|number| {
            classify_pr_updated_at(&workflow, number, &["implementation", "alignment"], fresh)
        })
        .collect();
    assert!(planner.queue_active(&queue, &at_depth, now));

    let old_enough = vec![classify_pr_updated_at(
        &workflow,
        42,
        &["implementation", "alignment"],
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
        parents: vec![ArtifactRef::same_repo(ItemNumber::new(2))],
        dependencies: vec![ArtifactRef::same_repo(ItemNumber::new(3))],
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
                target: ArtifactRef::same_repo(ItemNumber::new(2)),
                target_kinds: vec![ArtifactKindId::new("design"), ArtifactKindId::new("epic")],
            },
            ClassifiedRelation {
                kind: RelationKind::Dependency,
                source: ArtifactKindId::new("code"),
                target: ArtifactRef::same_repo(ItemNumber::new(3)),
                target_kinds: vec![ArtifactKindId::new("code")],
            },
        ]
    );

    let pr_body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(ItemNumber::new(1))],
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
            target: ArtifactRef::same_repo(ItemNumber::new(1)),
            target_kinds: vec![ArtifactKindId::new("code")],
        }]
    );
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
fn mechanical_landing_requires_review_and_native_ci() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    let ready = classify_pr(&workflow, 10, &["implementation", "landing"]);
    let review = GateSignals::new().with_review(ReviewStatus::new(true, false));
    let blocked = planner
        .plan_transition_with(
            &TransitionId::new("land_pr"),
            &RoleId::new("mechanical"),
            &ready,
            &review,
        )
        .expect_err("a merge cannot plan until the CI signal reports passed");
    assert!(blocked
        .diagnostics()
        .contains(&PlanDiagnostic::GateNotSatisfied {
            transition: TransitionId::new("land_pr"),
            gate: GateId::new("ci_gate"),
        }));

    let ci_only = GateSignals::new().with_ci(CiStatus::passed());
    let blocked = planner
        .plan_transition_with(
            &TransitionId::new("land_pr"),
            &RoleId::new("mechanical"),
            &ready,
            &ci_only,
        )
        .expect_err("a PR with landing and green CI still needs native approval");
    assert!(blocked
        .diagnostics()
        .contains(&PlanDiagnostic::GateNotSatisfied {
            transition: TransitionId::new("land_pr"),
            gate: GateId::new("review_gate"),
        }));

    let signals = GateSignals::new()
        .with_ci(CiStatus::passed())
        .with_review(ReviewStatus::new(true, false));
    let plan = planner
        .plan_transition_with(
            &TransitionId::new("land_pr"),
            &RoleId::new("mechanical"),
            &ready,
            &signals,
        )
        .expect("mechanical automation can land a fully gated PR once CI passes");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::MergePullRequest,
            WorkflowEffect::RemoveLabel(LabelId::new("landing")),
            WorkflowEffect::AddLabel(LabelId::new("landed")),
            WorkflowEffect::AddLabel(LabelId::new("alignment")),
        ]
    );
}

#[test]
fn fresh_implementation_pr_matches_reviewer_queue() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let fresh_pr = classify_pr(&workflow, 19, &["implementation", "needs-reviewer"]);

    assert!(planner
        .matching_queues(&fresh_pr)
        .contains(&QueueId::new("pr_needs_review")));
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

    let landing_failed = classify_pr(&workflow, 22, &["implementation", "landing"]);
    let return_for_review = planner
        .plan_transition(
            &TransitionId::new("address_landing_ci_failure"),
            &RoleId::new("engineer"),
            &landing_failed,
        )
        .expect("landing-approved CI failure returns to review");
    assert_eq!(
        return_for_review.effects,
        vec![
            WorkflowEffect::RemoveLabel(LabelId::new("landing")),
            WorkflowEffect::AddLabel(LabelId::new("needs-reviewer")),
            WorkflowEffect::RequestReviewers {
                roles: vec![RoleId::new("reviewer")],
            },
        ]
    );

    let conflicted = classify_pr(&workflow, 23, &["implementation", "merge-conflict"]);
    assert!(planner
        .matching_queues(&conflicted)
        .contains(&QueueId::new("pr_merge_conflict")));
    let requeue = planner
        .plan_transition(
            &TransitionId::new("resolve_merge_conflict"),
            &RoleId::new("engineer"),
            &conflicted,
        )
        .expect("engineer can requeue a conflict resolution without review request");
    assert_eq!(
        requeue.effects,
        vec![
            WorkflowEffect::RemoveLabel(LabelId::new("merge-conflict")),
            WorkflowEffect::AddLabel(LabelId::new("landing")),
        ]
    );
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

/// Looks up a compiled role tool's verdict -> transition outcome routing.
fn tool_outcome(
    compiled: &temper_workflow::CompiledWorkflow,
    role: &str,
    tool: &str,
    verdict: &str,
) -> Option<TransitionId> {
    compiled
        .roles()
        .iter()
        .find(|r| r.id.as_str() == role)
        .and_then(|r| r.tools.iter().find(|t| t.name == tool))
        .and_then(|t| t.outcomes.get(&VerdictId::new(verdict)).cloned())
}

#[test]
fn raw_human_intake_classifies_as_the_default_kind() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    // A freshly filed human issue carries no labels at all. The default `intake`
    // kind admits it as a normal work item, and the mechanical `mark_untriaged`
    // transition stamps it so the architect's `design_triage` queue can pick it
    // up.
    let raw = classify_issue(&workflow, 1, &[]);
    assert_eq!(raw.kind, ArtifactKindId::new("intake"));

    let stamp = planner
        .plan_transition(
            &TransitionId::new("mark_untriaged"),
            &RoleId::new("mechanical"),
            &raw,
        )
        .expect("mechanical automation can stamp raw intake untriaged");
    assert_eq!(
        stamp.effects,
        vec![WorkflowEffect::AddLabel(LabelId::new("untriaged"))]
    );

    // Once stamped, the same default-kind issue flows into architect triage.
    let stamped = classify_issue(&workflow, 1, &["untriaged"]);
    assert_eq!(stamped.kind, ArtifactKindId::new("intake"));
    assert!(planner
        .matching_queues(&stamped)
        .contains(&QueueId::new("design_triage")));
}

#[test]
fn architect_triage_intake_routes_verdicts_to_content_bearing_transitions() {
    let workflow = fixture_workflow();
    let compiled = compile(&workflow);
    let planner = workflow.planner();
    let architect = RoleId::new("architect");
    let intake = classify_issue(&workflow, 1, &["untriaged"]);

    // The workspace-backed triage action routes each declared verdict to its
    // content-bearing transition; the engine treats the verdict ids as opaque.
    assert_eq!(
        tool_outcome(&compiled, "architect", "triage_intake", "ready_code"),
        Some(TransitionId::new("triage_intake_to_code"))
    );
    assert_eq!(
        tool_outcome(&compiled, "architect", "triage_intake", "needs_design"),
        Some(TransitionId::new("triage_intake_to_design"))
    );
    assert_eq!(
        tool_outcome(&compiled, "architect", "triage_intake", "needs_breakdown"),
        Some(TransitionId::new("triage_intake_breakdown"))
    );

    // ready_code: rewrite the body into a crisp code spec, then code + ready.
    let to_code = planner
        .plan_transition(
            &TransitionId::new("triage_intake_to_code"),
            &architect,
            &intake,
        )
        .expect("architect can rewrite intake into ready code");
    assert_eq!(
        to_code.effects,
        vec![
            WorkflowEffect::SetBody {
                correlation_key: Some("triage-intake-code".to_string()),
            },
            WorkflowEffect::RemoveLabel(LabelId::new("untriaged")),
            WorkflowEffect::AddLabel(LabelId::new("code")),
            WorkflowEffect::AddLabel(LabelId::new("ready")),
        ]
    );

    // needs_design: author a design proposal body, then design + needs-owner.
    let to_design = planner
        .plan_transition(
            &TransitionId::new("triage_intake_to_design"),
            &architect,
            &intake,
        )
        .expect("architect can rewrite intake into a design proposal");
    assert_eq!(
        to_design.effects,
        vec![
            WorkflowEffect::SetBody {
                correlation_key: Some("triage-intake-design".to_string()),
            },
            WorkflowEffect::RemoveLabel(LabelId::new("untriaged")),
            WorkflowEffect::AddLabel(LabelId::new("design")),
            WorkflowEffect::AddLabel(LabelId::new("needs-owner")),
        ]
    );

    // needs_breakdown: create dependent children; the parent becomes a plan
    // record (an epic).
    let breakdown = planner
        .plan_transition(
            &TransitionId::new("triage_intake_breakdown"),
            &architect,
            &intake,
        )
        .expect("architect can break intake into dependent children");
    assert_eq!(
        breakdown.effects,
        vec![
            WorkflowEffect::CreateIssues {
                correlation_key: Some("triage-intake-breakdown".to_string()),
            },
            WorkflowEffect::RemoveLabel(LabelId::new("untriaged")),
            WorkflowEffect::AddLabel(LabelId::new("epic")),
        ]
    );
}

#[test]
fn reviewer_review_pr_routes_to_native_review_and_escalation() {
    let workflow = fixture_workflow();
    let compiled = compile(&workflow);
    let planner = workflow.planner();
    let reviewer = RoleId::new("reviewer");
    let pr = classify_pr(&workflow, 10, &["implementation", "needs-reviewer"]);

    // The reviewer workspace reads the real diff/CI and routes its verdict: an
    // approval queues landing, a changes verdict attaches a native review with
    // the authored body, and an escalation flags the architect.
    assert_eq!(
        tool_outcome(&compiled, "reviewer", "review_pr", "approve"),
        Some(TransitionId::new("approve_review"))
    );
    assert_eq!(
        tool_outcome(&compiled, "reviewer", "review_pr", "changes"),
        Some(TransitionId::new("request_changes_with_review"))
    );
    assert_eq!(
        tool_outcome(&compiled, "reviewer", "review_pr", "escalate"),
        Some(TransitionId::new("request_architect_input"))
    );

    // The changes route carries the authored review body into a native review.
    let changes = planner
        .plan_transition(
            &TransitionId::new("request_changes_with_review"),
            &reviewer,
            &pr,
        )
        .expect("reviewer can request changes with an attached review");
    assert_eq!(
        changes.effects,
        vec![
            WorkflowEffect::RemoveLabel(LabelId::new("needs-reviewer")),
            WorkflowEffect::AttachReview {
                decision: ReviewDecision::ChangesRequested,
                correlation_key: Some("review-changes".to_string()),
            },
        ]
    );
}
