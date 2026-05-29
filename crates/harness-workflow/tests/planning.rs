//! Tests for pure queue evaluation and transition planning (Phase 5).
//!
//! These exercise the state machine without any Forge side effects: classify a
//! Forge artifact, then match queues and plan transitions over the validated
//! workflow. The checked-in five-role fixture drives the realistic cases;
//! small inline workflows drive the impossible-state edge case.

use chrono::{DateTime, Utc};
use harness_forge::{BranchRef, Issue, IssueState, ItemNumber, PullRequest, PullRequestState};
use harness_workflow::{
    compile, matches_queue, ArtifactKindId, CiStatus, ClassifiedArtifact, Classifier, GateId,
    GateSignals, LabelId, PlanDiagnostic, Postcondition, QueueId, RawWorkflowSpec, RoleId,
    StateDimensionId, StateId, TransitionId, ValidatedWorkflow, WorkflowEffect,
};

/// The checked-in five-role delivery workflow fixture.
const FIXTURE: &str = include_str!("../fixtures/five-role-delivery.json");

fn ts() -> DateTime<Utc> {
    "2026-05-29T00:00:00Z".parse().expect("valid timestamp")
}

fn fixture_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(FIXTURE).expect("fixture is valid JSON for RawWorkflowSpec");
    spec.validate().expect("five-role fixture validates")
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

#[test]
fn queue_matching_selects_code_ready_and_excludes_others() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    let artifacts = vec![
        classify_issue(&workflow, 1, &["code", "ready"]),
        classify_issue(&workflow, 2, &["code", "blocked"]),
        classify_issue(&workflow, 3, &["code", "in-progress"]),
        classify_issue(&workflow, 4, &["design", "design-draft"]),
    ];

    let members = planner.queue_members(&QueueId::new("code_ready"), &artifacts);
    let numbers: Vec<u64> = members
        .iter()
        .map(|artifact| match artifact.source {
            harness_workflow::ArtifactSource::Issue { number } => number.get(),
            harness_workflow::ArtifactSource::PullRequest { number } => number.get(),
        })
        .collect();

    // Only the `code + ready` issue is selected; blocked and in-progress code
    // issues and the design issue are excluded.
    assert_eq!(numbers, vec![1]);

    // The ready issue reports the queue, the blocked one does not.
    assert!(planner
        .matching_queues(&artifacts[0])
        .contains(&QueueId::new("code_ready")));
    assert!(!planner
        .matching_queues(&artifacts[1])
        .contains(&QueueId::new("code_ready")));
}

#[test]
fn queue_matching_works_against_a_compiled_manifest() {
    let workflow = fixture_workflow();
    let compiled = compile(&workflow);
    let code_ready = compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "code_ready")
        .expect("code_ready queue is compiled");

    let ready = classify_issue(&workflow, 1, &["code", "ready"]);
    let blocked = classify_issue(&workflow, 2, &["code", "blocked"]);

    // The same matcher works against the compiled QueueManifest via QueueQuery.
    assert!(matches_queue(code_ready, &ready));
    assert!(!matches_queue(code_ready, &blocked));
}

#[test]
fn engineer_can_plan_claim_code_but_reviewer_cannot() {
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
    assert_eq!(plan.transition, TransitionId::new("claim_code"));
    assert_eq!(plan.artifact, ArtifactKindId::new("code"));

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
fn planning_fails_when_preconditions_are_stale_or_contradicted() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    // An issue already in progress: claiming it again is stale (no `ready` to
    // remove) and contradicted (`in-progress` already present).
    let artifact = classify_issue(&workflow, 7, &["code", "in-progress"]);
    let error = planner
        .plan_transition(
            &TransitionId::new("claim_code"),
            &RoleId::new("engineer"),
            &artifact,
        )
        .expect_err("a re-claim must not plan");

    assert!(error
        .diagnostics()
        .contains(&PlanDiagnostic::StalePrecondition {
            transition: TransitionId::new("claim_code"),
            label: LabelId::new("ready"),
        }));
    assert!(error
        .diagnostics()
        .contains(&PlanDiagnostic::ContradictedPrecondition {
            transition: TransitionId::new("claim_code"),
            label: LabelId::new("in-progress"),
        }));
}

#[test]
fn planned_effects_and_postconditions_are_deterministic() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let artifact = classify_issue(&workflow, 42, &["code", "ready"]);

    let first = planner
        .plan_transition(
            &TransitionId::new("claim_code"),
            &RoleId::new("engineer"),
            &artifact,
        )
        .expect("claim_code plans");
    let second = planner
        .plan_transition(
            &TransitionId::new("claim_code"),
            &RoleId::new("engineer"),
            &artifact,
        )
        .expect("claim_code plans again");

    // Re-planning the same input yields an identical plan.
    assert_eq!(first, second);

    // Effects follow the transition's declared order, mapped to typed effects.
    assert_eq!(
        first.effects,
        vec![
            WorkflowEffect::RemoveLabel(LabelId::new("ready")),
            WorkflowEffect::AddLabel(LabelId::new("in-progress")),
        ]
    );
    // Postconditions mirror the effects in the same order.
    assert_eq!(
        first.postconditions,
        vec![
            Postcondition::LabelAbsent(LabelId::new("ready")),
            Postcondition::LabelPresent(LabelId::new("in-progress")),
        ]
    );
}

#[test]
fn required_gates_must_be_satisfied_before_planning_a_merge() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    // Both gates satisfied: review approved and testing passed.
    let ready = classify_pr(
        &workflow,
        10,
        &["implementation", "review-approved", "testing-passed"],
    );
    let plan = planner
        .plan_transition(
            &TransitionId::new("approve_merge"),
            &RoleId::new("owner"),
            &ready,
        )
        .expect("owner can approve a fully gated merge");
    // Merge eligibility is derived from the gates, not a stored marker: the
    // transition merges the pull request and projects the post-merge labels
    // (the `landed` re-run guard) in declaration order.
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::MergePullRequest,
            WorkflowEffect::AddLabel(LabelId::new("landed")),
            WorkflowEffect::AddLabel(LabelId::new("owner-pending")),
        ]
    );

    // Testing gate unsatisfied: the merge cannot be planned.
    let pending = classify_pr(&workflow, 11, &["implementation", "review-approved"]);
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
            gate: GateId::new("testing_gate"),
        }));
}

#[test]
fn ci_gate_requires_runtime_ci_signal_before_merge_plans() {
    let json = r#"{
        "name": "ci-gated-merge",
        "roles": [{"id": "owner", "queues": []}],
        "labels": [{"id": "implementation"}, {"id": "landed"}],
        "artifact_kinds": [
            {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
        ],
        "transitions": [
            {"id": "approve_merge", "artifact": "implementation_pr", "roles": ["owner"],
             "requires_gates": ["ci_gate"], "effects": [
                {"kind": "merge_pull_request"},
                {"kind": "add_label", "label": "landed"}
             ]}
        ],
        "gates": [{"id": "ci_gate", "condition": {"kind": "ci_passed"}}]
    }"#;
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("json parses");
    let workflow = spec.validate().expect("workflow validates");
    let planner = workflow.planner();
    let artifact = classify_pr(&workflow, 10, &["implementation"]);

    let blocked = planner
        .plan_transition(
            &TransitionId::new("approve_merge"),
            &RoleId::new("owner"),
            &artifact,
        )
        .expect_err("CI has not passed by default");
    assert!(blocked
        .diagnostics()
        .contains(&PlanDiagnostic::GateNotSatisfied {
            transition: TransitionId::new("approve_merge"),
            gate: GateId::new("ci_gate"),
        }));

    let signals = GateSignals::new().with_ci(CiStatus::passed());
    let plan = planner
        .plan_transition_with(
            &TransitionId::new("approve_merge"),
            &RoleId::new("owner"),
            &artifact,
            &signals,
        )
        .expect("a passed CI signal opens the gate");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::MergePullRequest,
            WorkflowEffect::AddLabel(LabelId::new("landed")),
        ]
    );
}

#[test]
fn unknown_transition_and_artifact_kind_mismatch_are_reported() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let code = classify_issue(&workflow, 1, &["code", "ready"]);
    let pr = classify_pr(&workflow, 2, &["implementation", "needs-review"]);

    let unknown = planner
        .plan_transition(
            &TransitionId::new("does_not_exist"),
            &RoleId::new("engineer"),
            &code,
        )
        .expect_err("an undeclared transition cannot plan");
    assert_eq!(
        unknown.diagnostics(),
        &[PlanDiagnostic::UnknownTransition {
            transition: TransitionId::new("does_not_exist"),
        }]
    );

    // claim_code acts on `code`, but here it is aimed at an implementation PR.
    let mismatch = planner
        .plan_transition(
            &TransitionId::new("claim_code"),
            &RoleId::new("engineer"),
            &pr,
        )
        .expect_err("a kind mismatch cannot plan");
    assert!(mismatch
        .diagnostics()
        .contains(&PlanDiagnostic::ArtifactKindMismatch {
            transition: TransitionId::new("claim_code"),
            expected: ArtifactKindId::new("code"),
            actual: ArtifactKindId::new("implementation_pr"),
        }));
}

#[test]
fn impossible_state_dimensions_are_diagnosed_before_planning() {
    // A deliberately under-specified transition: it adds `in-progress` without
    // removing `ready`. Static validation does not yet reject this, so the
    // planner must catch that applying it to a ready issue would leave the
    // exclusive lifecycle dimension in two states at once.
    let json = r#"{
        "name": "impossible-demo",
        "roles": [{"id": "engineer", "queues": []}],
        "labels": [{"id": "code"}, {"id": "ready"}, {"id": "in-progress"}],
        "artifact_kinds": [
            {"id": "code", "target": "issue", "identifying_labels": ["code"]}
        ],
        "state_dimensions": [
            {"id": "code_lifecycle", "exclusive": true, "states": [
                {"id": "ready", "label": "ready"},
                {"id": "in_progress", "label": "in-progress"}
            ]}
        ],
        "transitions": [
            {"id": "bad_claim", "artifact": "code", "roles": ["engineer"], "effects": [
                {"kind": "add_label", "label": "in-progress"}
            ]}
        ]
    }"#;
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("json parses");
    let workflow = spec.validate().expect("workflow validates");
    let planner = workflow.planner();
    let artifact = classify_issue(&workflow, 1, &["code", "ready"]);

    let error = planner
        .plan_transition(
            &TransitionId::new("bad_claim"),
            &RoleId::new("engineer"),
            &artifact,
        )
        .expect_err("a transition that creates an impossible state must not plan");

    assert!(error.diagnostics().iter().any(|diagnostic| matches!(
        diagnostic,
        PlanDiagnostic::ImpossibleState { dimension, states, .. }
            if dimension == &StateDimensionId::new("code_lifecycle")
                && states.contains(&StateId::new("ready"))
                && states.contains(&StateId::new("in_progress"))
    )));
}
