//! Confirmation tests for the `basic-delivery` workflow fixture.
//!
//! `basic-delivery` is the minimal, no-human-in-the-loop reference shape: a
//! single unlabeled intake issue is stamped `untriaged` by the bot, triaged by
//! the architect into a ready code issue (a *single* outcome — no design or
//! breakdown branch), implemented by the engineer into a PR, and auto-merged by
//! the bot once CI is green. Only three workers participate: `architect`,
//! `engineer`, and `mechanical`. There is no reviewer/owner/human role and the
//! landing gate is CI-only (no review gate).
//!
//! This mirrors `reference_delivery.rs` at smaller scope. It is the in-process
//! fixture check; it exercises validation, classification, queue matching, and
//! transition planning against the fixture, not a live Forgejo run.
//!
//! Note: the `intake_author` workflow knob (the `{ "kind": "site_admin" }`
//! sketch in the parent plan) is owned by sibling task W2 and is not yet a field
//! of `RawWorkflowSpec` (which is `deny_unknown_fields`). The fixture therefore
//! omits it for now; W2 adds the field and updates the fixture to set it.

use temper_forge::{BranchRef, Issue, IssueState, ItemNumber, PullRequest, PullRequestState};
use temper_workflow::{
    compile, ArtifactKindId, CiStatus, ClassifiedArtifact, Classifier, GateCondition, GateId,
    GateSignals, LabelId, PlanDiagnostic, QueueId, RawWorkflowSpec, ReviewStatus, RoleId,
    TransitionId, ValidatedWorkflow, VerdictId, WorkflowEffect,
};

const FIXTURE: &str = include_str!("../fixtures/basic-delivery.json");

fn ts() -> chrono::DateTime<chrono::Utc> {
    "2026-05-29T00:00:00Z".parse().expect("valid timestamp")
}

fn fixture_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(FIXTURE).expect("fixture is valid JSON for RawWorkflowSpec");
    spec.validate().expect("basic-delivery fixture validates")
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
fn basic_delivery_fixture_validates_with_expected_shape() {
    let workflow = fixture_workflow();
    assert_eq!(workflow.name(), "basic-delivery");

    // Exactly three roles drive the run: architect, engineer, mechanical. No
    // reviewer, owner, or human role exists in this minimal shape.
    let mut role_ids: Vec<&str> = workflow.roles().iter().map(|r| r.id.as_str()).collect();
    role_ids.sort_unstable();
    assert_eq!(role_ids, vec!["architect", "engineer", "mechanical"]);
    assert!(
        !role_ids.contains(&"reviewer")
            && !role_ids.contains(&"owner")
            && !role_ids.contains(&"human"),
        "basic-delivery has no reviewer/owner/human role"
    );

    // The mechanical authority draws no queues of its own; it is serviced by the
    // mechanical worker via queue automation.
    let mechanical = workflow
        .roles()
        .iter()
        .find(|role| role.id.as_str() == "mechanical")
        .expect("mechanical automation authority is declared");
    assert!(mechanical.queues.is_empty());

    // `intake` is the default (catch-all) issue kind: no identifying labels, so a
    // freshly filed unlabeled issue is admitted as intake.
    let intake = workflow
        .artifact_kinds()
        .iter()
        .find(|kind| kind.id.as_str() == "intake")
        .expect("intake kind is declared");
    assert!(
        intake.identifying_labels.is_empty(),
        "intake is the default issue kind and carries no identifying labels"
    );
}

#[test]
fn basic_delivery_compiles_only_the_three_workers() {
    let compiled = compile(&fixture_workflow());
    let mut ids: Vec<String> = compiled.roles().iter().map(|r| r.id.to_string()).collect();
    ids.sort();
    assert_eq!(ids, vec!["architect", "engineer", "mechanical"]);
}

#[test]
fn raw_intake_is_stamped_untriaged_then_flows_to_architect_triage() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    // A freshly filed issue carries no labels. The default `intake` kind admits
    // it, and the `raw_intake` queue drives the mechanical `mark_untriaged`
    // stamp.
    let raw = classify_issue(&workflow, 1, &[]);
    assert_eq!(raw.kind, ArtifactKindId::new("intake"));

    let raw_intake = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "raw_intake")
        .expect("raw_intake mechanical queue is declared");
    assert!(raw_intake.labels.is_empty());
    let raw_automation = raw_intake
        .automation
        .as_ref()
        .expect("raw_intake queue is mechanically serviced");
    assert_eq!(raw_automation.actor, RoleId::new("mechanical"));
    assert_eq!(
        raw_automation.transition,
        TransitionId::new("mark_untriaged")
    );

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

    // Once stamped, the same default-kind issue flows into the architect's triage
    // queue.
    let stamped = classify_issue(&workflow, 1, &["untriaged"]);
    assert_eq!(stamped.kind, ArtifactKindId::new("intake"));
    assert!(planner
        .matching_queues(&stamped)
        .contains(&QueueId::new("triage")));
}

#[test]
fn architect_triage_has_a_single_outcome_into_ready_code() {
    let workflow = fixture_workflow();
    let compiled = compile(&workflow);
    let planner = workflow.planner();
    let architect = RoleId::new("architect");
    let intake = classify_issue(&workflow, 1, &["untriaged"]);

    // The triage action exposes exactly one declared verdict: `ready_code`. There
    // is no `needs_design` or `needs_breakdown` branch in the minimal shape.
    let triage_intake = workflow
        .transitions()
        .iter()
        .find(|transition| transition.id.as_str() == "triage_intake")
        .expect("triage_intake transition is declared");
    assert_eq!(
        triage_intake.outcomes.len(),
        1,
        "triage has a single declared outcome"
    );
    assert_eq!(
        triage_intake.outcomes.get(&VerdictId::new("ready_code")),
        Some(&TransitionId::new("triage_intake_to_code"))
    );

    // The single verdict routes through the compiled tool to the content-bearing
    // transition.
    assert_eq!(
        tool_outcome(&compiled, "architect", "triage_intake", "ready_code"),
        Some(TransitionId::new("triage_intake_to_code"))
    );
    assert!(
        tool_outcome(&compiled, "architect", "triage_intake", "needs_design").is_none(),
        "needs_design is not a declared verdict in basic-delivery"
    );
    assert!(
        tool_outcome(&compiled, "architect", "triage_intake", "needs_breakdown").is_none(),
        "needs_breakdown is not a declared verdict in basic-delivery"
    );

    // ready_code rewrites the body into a crisp code spec, then code + ready.
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
}

#[test]
fn engineer_claims_ready_code_and_opens_a_pr() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let ready_code = classify_issue(&workflow, 42, &["code", "ready"]);

    // Ready code lands in the engineer's `code_ready` queue.
    assert!(planner
        .matching_queues(&ready_code)
        .contains(&QueueId::new("code_ready")));

    // `open_pr` claims the issue, assigns the engineer, and creates a PR.
    let open_pr = planner
        .plan_transition(
            &TransitionId::new("open_pr"),
            &RoleId::new("engineer"),
            &ready_code,
        )
        .expect("engineer can open a PR for ready code");
    assert_eq!(
        open_pr.effects,
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

    // The architect, who has no code authority here, cannot open a PR.
    let error = planner
        .plan_transition(
            &TransitionId::new("open_pr"),
            &RoleId::new("architect"),
            &ready_code,
        )
        .expect_err("only the engineer may open PRs");
    assert!(error.diagnostics().contains(&PlanDiagnostic::Unauthorized {
        transition: TransitionId::new("open_pr"),
        role: RoleId::new("architect"),
    }));
}

#[test]
fn mechanical_landing_gate_is_ci_only_no_review_required() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    // The landing gate is CI-only: `ci_gate` checks `ci_passed`, and `land_pr`
    // requires only that gate. There is no review gate in basic-delivery.
    let ci_gate = workflow
        .gates()
        .iter()
        .find(|gate| gate.id.as_str() == "ci_gate")
        .expect("ci_gate is declared");
    assert_eq!(ci_gate.condition.as_ref(), Some(&GateCondition::CiPassed));
    assert_eq!(workflow.gates().len(), 1, "the only gate is the CI gate");

    let land_pr = workflow
        .transitions()
        .iter()
        .find(|transition| transition.id.as_str() == "land_pr")
        .expect("land_pr transition is declared");
    assert_eq!(
        land_pr.requires_gates,
        vec![GateId::new("ci_gate")],
        "landing requires the CI gate only — no review gate"
    );

    // The `landing` queue is mechanically serviced once CI passes.
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

    // A PR with green CI lands with no review signal supplied at all — proving the
    // gate set does not include review approval.
    let ready = classify_pr(&workflow, 10, &["implementation"]);
    let ci_only = GateSignals::new().with_ci(CiStatus::passed());
    let plan = planner
        .plan_transition_with(
            &TransitionId::new("land_pr"),
            &RoleId::new("mechanical"),
            &ready,
            &ci_only,
        )
        .expect("mechanical automation can land a PR on CI alone");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::MergePullRequest,
            WorkflowEffect::AddLabel(LabelId::new("landed")),
        ]
    );

    // Without CI, landing is gated even though no review is ever required.
    let no_ci = GateSignals::new();
    let blocked = planner
        .plan_transition_with(
            &TransitionId::new("land_pr"),
            &RoleId::new("mechanical"),
            &ready,
            &no_ci,
        )
        .expect_err("landing is blocked until CI passes");
    assert!(blocked
        .diagnostics()
        .contains(&PlanDiagnostic::GateNotSatisfied {
            transition: TransitionId::new("land_pr"),
            gate: GateId::new("ci_gate"),
        }));
}

#[test]
fn failed_ci_routes_back_to_the_engineer() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    // A PR whose CI failed lands in the engineer's `pr_ci_failed` queue.
    let failed = classify_pr(&workflow, 21, &["implementation"]);
    let ci_failed = GateSignals::new().with_ci(CiStatus::failed());
    assert!(planner
        .matching_queues_with(&failed, &ci_failed)
        .contains(&QueueId::new("pr_ci_failed")));

    // The engineer can act on it via `address_ci_failure`.
    let respond = planner
        .plan_transition(
            &TransitionId::new("address_ci_failure"),
            &RoleId::new("engineer"),
            &failed,
        )
        .expect("engineer can address a CI failure");
    assert!(matches!(
        respond.effects.as_slice(),
        [WorkflowEffect::CreateComment { .. }]
    ));

    // The same green PR does not match the CI-failure queue.
    let green = classify_pr(&workflow, 22, &["implementation"]);
    let ci_passed = GateSignals::new().with_ci(CiStatus::passed());
    assert!(!planner
        .matching_queues_with(&green, &ci_passed)
        .contains(&QueueId::new("pr_ci_failed")));
}

#[test]
fn review_status_unused_keeps_landing_unblocked() {
    // Defensive: even if a stray review signal is present, the CI-only gate does
    // not consult it; landing depends solely on CI. This documents that adding a
    // review signal neither helps nor blocks a basic-delivery landing.
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let ready = classify_pr(&workflow, 30, &["implementation"]);

    let ci_and_stray_review = GateSignals::new()
        .with_ci(CiStatus::passed())
        .with_review(ReviewStatus::new(false, false));
    let plan = planner
        .plan_transition_with(
            &TransitionId::new("land_pr"),
            &RoleId::new("mechanical"),
            &ready,
            &ci_and_stray_review,
        )
        .expect("landing depends on CI only; an unapproved review does not block it");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::MergePullRequest,
            WorkflowEffect::AddLabel(LabelId::new("landed")),
        ]
    );
}
