//! Tests for pure queue evaluation and transition planning (Phase 5).
//!
//! These exercise the state machine without any Forge side effects: classify a
//! Forge artifact, then match queues and plan transitions over the validated
//! workflow. The checked-in CI delivery fixture drives the realistic cases;
//! small inline workflows drive the impossible-state edge case.

use chrono::{DateTime, Utc};
use temper_forge::{
    BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, Issue, IssueState, ItemNumber,
    PullRequest, PullRequestState, RepositoryId,
};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, CiState, CiStatus, ClassifiedArtifact, Classifier,
    DependencyStatus, GateId, GateSignals, LabelId, PlanDiagnostic, Postcondition, QueueId,
    RawWorkflowSpec, ReviewStatus, RoleId, StateDimensionId, StateId, TransitionId,
    ValidatedWorkflow, WorkflowEffect, compile, matches_queue,
};

/// The checked-in CI delivery workflow fixture.
const FIXTURE: &str = include_str!("../fixtures/ci-delivery.json");

fn ts() -> DateTime<Utc> {
    "2026-05-29T00:00:00Z".parse().expect("valid timestamp")
}

fn fixture_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(FIXTURE).expect("fixture is valid JSON for RawWorkflowSpec");
    spec.validate().expect("CI delivery fixture validates")
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

fn classify_issue_with_dependencies(
    workflow: &ValidatedWorkflow,
    number: u64,
    labels: &[&str],
    dependencies: &[u64],
) -> ClassifiedArtifact {
    Classifier::new(workflow)
        .classify_issue(&issue_with_dependencies(number, labels, dependencies))
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
            temper_workflow::ArtifactSource::Issue { number } => number.get(),
            temper_workflow::ArtifactSource::PullRequest { number } => number.get(),
        })
        .collect();

    // Only the `code + ready` issue is selected; blocked and in-progress code
    // issues and the design issue are excluded.
    assert_eq!(numbers, vec![1]);

    // The ready issue reports the queue, the blocked one does not.
    assert!(
        planner
            .matching_queues(&artifacts[0])
            .contains(&QueueId::new("code_ready"))
    );
    assert!(
        !planner
            .matching_queues(&artifacts[1])
            .contains(&QueueId::new("code_ready"))
    );
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

    assert!(
        error
            .diagnostics()
            .contains(&PlanDiagnostic::StalePrecondition {
                transition: TransitionId::new("claim_code"),
                label: LabelId::new("ready"),
            })
    );
    assert!(
        error
            .diagnostics()
            .contains(&PlanDiagnostic::ContradictedPrecondition {
                transition: TransitionId::new("claim_code"),
                label: LabelId::new("in-progress"),
            })
    );
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
    let ready = classify_pr(&workflow, 10, &["implementation"]);
    let review = GateSignals::new().with_review(ReviewStatus::new(true, false));

    let error = planner
        .plan_transition_with(
            &TransitionId::new("approve_merge"),
            &RoleId::new("owner"),
            &ready,
            &review,
        )
        .expect_err("CI must pass before a merge plans");
    assert!(
        error
            .diagnostics()
            .contains(&PlanDiagnostic::GateNotSatisfied {
                transition: TransitionId::new("approve_merge"),
                gate: GateId::new("ci_gate"),
            })
    );

    let signals = review.with_ci(CiStatus::passed());
    let plan = planner
        .plan_transition_with(
            &TransitionId::new("approve_merge"),
            &RoleId::new("owner"),
            &ready,
            &signals,
        )
        .expect("owner can approve a fully gated merge");
    // Merge eligibility is derived from the gates, not a stored marker.
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::MergePullRequest,
            WorkflowEffect::AddLabel(LabelId::new("landed")),
            WorkflowEffect::AddLabel(LabelId::new("alignment")),
        ]
    );
}

fn ci_job(
    name: &str,
    commit_sha: &str,
    status: CiJobStatus,
    conclusion: Option<CiJobConclusion>,
) -> CiJob {
    CiJob {
        id: CiJobId::new(format!("ci-{name}-{commit_sha}")),
        repo_id: "repo-1".into(),
        pull_request_id: None,
        commit_sha: commit_sha.into(),
        name: name.into(),
        status,
        conclusion,
        provider_conclusion: None,
        provider_reason: None,
        run_id: None,
        attempt: None,
        url: None,
        created_at: ts(),
        started_at: None,
        completed_at: None,
        updated_at: ts(),
    }
}

#[test]
fn ci_status_distinguishes_pending_passed_failed_and_recovery_required_jobs() {
    assert_eq!(CiStatus::from_jobs(&[]).state(), CiState::Pending);
    assert_eq!(
        CiStatus::from_jobs(&[ci_job("ci", "sha", CiJobStatus::Running, None)]).state(),
        CiState::Pending
    );
    assert_eq!(
        CiStatus::from_jobs(&[ci_job(
            "ci",
            "sha",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Success),
        )])
        .state(),
        CiState::Passed
    );
    assert_eq!(
        CiStatus::from_jobs(&[ci_job(
            "ci",
            "sha",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Failure),
        )])
        .state(),
        CiState::Failed
    );
    for conclusion in [
        CiJobConclusion::Cancelled,
        CiJobConclusion::Interrupted,
        CiJobConclusion::TimedOut,
        CiJobConclusion::RunnerLost,
        CiJobConclusion::StartupFailure,
        CiJobConclusion::ActionRequired,
        CiJobConclusion::Neutral,
        CiJobConclusion::Skipped,
        CiJobConclusion::Unknown,
    ] {
        assert_eq!(
            CiStatus::from_jobs(&[ci_job(
                "ci",
                "sha",
                CiJobStatus::Completed,
                Some(conclusion),
            )])
            .state(),
            CiState::RecoveryRequired,
            "{conclusion:?} must not route to code repair"
        );
    }
    assert_eq!(
        CiStatus::from_jobs(&[ci_job("ci", "sha", CiJobStatus::Completed, None)]).state(),
        CiState::RecoveryRequired,
        "ambiguous terminalization has no ordinary failure evidence"
    );
}

#[test]
fn ci_status_for_head_accepts_only_safe_sha_ownership_evidence() {
    let head = "ABCDEF0123456789ABCDEF0123456789ABCDEF01";
    let success = |sha| {
        ci_job(
            "ci",
            sha,
            CiJobStatus::Completed,
            Some(CiJobConclusion::Success),
        )
    };

    assert_eq!(
        CiStatus::from_jobs_for_head(
            &[success("1111111111111111111111111111111111111111")],
            Some(head)
        )
        .state(),
        CiState::Pending,
        "success owned only by an old head is not current-head evidence"
    );
    assert_eq!(
        CiStatus::from_jobs_for_head(&[success("abcdef0")], Some(head)).state(),
        CiState::Passed,
        "a safe case-insensitive job abbreviation identifies the full head"
    );
    assert_eq!(
        CiStatus::from_jobs_for_head(&[success(head)], Some("abcdef0")).state(),
        CiState::Passed,
        "a safe abbreviated provider head identifies a full job SHA"
    );
    assert_eq!(
        CiStatus::from_jobs_for_head(&[success("abcdef")], Some(head)).state(),
        CiState::Pending,
        "a prefix shorter than seven characters is unsafe"
    );
    assert_eq!(
        CiStatus::from_jobs_for_head(&[success("")], Some(head)).state(),
        CiState::Pending,
        "an empty job SHA is not ownership evidence"
    );
    assert_eq!(
        CiStatus::from_jobs_for_head(&[success("SHA")], Some("sha")).state(),
        CiState::Passed,
        "an exact case-insensitive match is accepted at any length"
    );
    assert_eq!(
        CiStatus::from_jobs_for_head(&[success("old-head")], Some("")).state(),
        CiState::Passed,
        "an empty provider head preserves unscoped aggregation"
    );
    assert_eq!(
        CiStatus::from_jobs_for_head(&[success("old-head")], None).state(),
        CiState::Passed,
        "an absent provider head preserves unscoped aggregation"
    );
}

#[test]
fn ci_status_filters_to_current_head_before_latest_job_aggregation() {
    let head = "abcdef0123456789abcdef0123456789abcdef01";
    let old_success = ci_job(
        "validate",
        "1111111111111111111111111111111111111111",
        CiJobStatus::Completed,
        Some(CiJobConclusion::Success),
    );
    let current_queued = ci_job("validate", head, CiJobStatus::Queued, None);

    assert_eq!(
        CiStatus::from_jobs_for_head(&[old_success.clone()], Some(head)).state(),
        CiState::Pending
    );
    assert_eq!(
        CiStatus::from_jobs_for_head(&[old_success.clone(), current_queued], Some(head)).state(),
        CiState::Pending,
        "queued current-head work remains pending despite old success"
    );

    let current_success = ci_job(
        "validate",
        head,
        CiJobStatus::Completed,
        Some(CiJobConclusion::Success),
    );
    assert_eq!(
        CiStatus::from_jobs_for_head(&[current_success, old_success], Some(head)).state(),
        CiState::Passed,
        "completed successful current-head work is not replaced by a stale job"
    );
}

#[test]
fn ci_status_completion_comes_only_from_terminal_latest_jobs() {
    let at = |value: &str| -> DateTime<Utc> { value.parse().expect("valid timestamp") };
    let head = "abcdef0123456789abcdef0123456789abcdef01";

    let mut old_attempt = ci_job(
        "build",
        head,
        CiJobStatus::Completed,
        Some(CiJobConclusion::Failure),
    );
    old_attempt.created_at = at("2026-05-29T00:00:01Z");
    old_attempt.completed_at = Some(at("2026-05-29T00:10:00Z"));

    let mut latest_build = ci_job(
        "build",
        head,
        CiJobStatus::Completed,
        Some(CiJobConclusion::Success),
    );
    latest_build.created_at = at("2026-05-29T00:00:02Z");
    latest_build.completed_at = Some(at("2026-05-29T00:01:02Z"));

    let mut latest_test = ci_job(
        "test",
        head,
        CiJobStatus::Completed,
        Some(CiJobConclusion::Success),
    );
    latest_test.created_at = at("2026-05-29T00:00:03Z");
    latest_test.completed_at = Some(at("2026-05-29T00:01:03Z"));

    let terminal = CiStatus::from_jobs_for_head(
        &[old_attempt, latest_build, latest_test.clone()],
        Some(head),
    );
    assert_eq!(terminal.state(), CiState::Passed);
    assert_eq!(
        terminal.completed_at(),
        Some(at("2026-05-29T00:01:03Z")),
        "an older attempt cannot move aggregate completion later"
    );

    let running = ci_job("lint", head, CiJobStatus::Running, None);
    let pending = CiStatus::from_jobs_for_head(&[latest_test.clone(), running], Some(head));
    assert_eq!(pending.state(), CiState::Pending);
    assert_eq!(pending.completed_at(), None);

    latest_test.completed_at = None;
    let terminal_without_complete_timestamps =
        CiStatus::from_jobs_for_head(&[latest_test], Some(head));
    assert_eq!(
        terminal_without_complete_timestamps.state(),
        CiState::Passed
    );
    assert_eq!(terminal_without_complete_timestamps.completed_at(), None);
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
    assert!(
        blocked
            .diagnostics()
            .contains(&PlanDiagnostic::GateNotSatisfied {
                transition: TransitionId::new("approve_merge"),
                gate: GateId::new("ci_gate"),
            })
    );

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
fn dependency_gate_requires_declared_relations_and_landed_targets() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let missing = classify_issue(&workflow, 39, &["code", "blocked"]);

    let error = planner
        .plan_transition_with(
            &TransitionId::new("mark_code_ready"),
            &RoleId::new("architect"),
            &missing,
            &GateSignals::new(),
        )
        .expect_err("an empty dependency set must fail closed");
    assert!(
        error
            .diagnostics()
            .contains(&PlanDiagnostic::GateNotSatisfied {
                transition: TransitionId::new("mark_code_ready"),
                gate: temper_workflow::GateId::new("dependency_gate"),
            })
    );

    let blocked = classify_issue_with_dependencies(&workflow, 40, &["code", "blocked"], &[41]);

    assert!(
        planner
            .dependency_unblocks(&blocked, &DependencyStatus::default())
            .is_empty()
    );

    let landed = DependencyStatus::landed([ItemNumber::new(41)]);
    let plan = planner
        .plan_transition_with(
            &TransitionId::new("mark_code_ready"),
            &RoleId::new("architect"),
            &blocked,
            &GateSignals::new().with_dependencies(landed),
        )
        .expect("native dependency link opens the gate once landed");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::RemoveLabel(LabelId::new("blocked")),
            WorkflowEffect::AddLabel(LabelId::new("ready")),
        ]
    );
}

#[test]
fn dependency_status_distinguishes_same_repo_and_cross_repo_targets() {
    let same_repo = ArtifactRef::same_repo(ItemNumber::new(41));
    let cross_repo = ArtifactRef::in_repo(RepositoryId::new("repo-other"), ItemNumber::new(41));

    let same_only = DependencyStatus::landed([same_repo.clone()]);
    assert!(same_only.is_landed(&same_repo));
    assert!(!same_only.is_landed(&cross_repo));

    let cross_only = DependencyStatus::landed([cross_repo.clone()]);
    assert!(!cross_only.is_landed(&same_repo));
    assert!(cross_only.is_landed(&cross_repo));
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
    assert!(
        mismatch
            .diagnostics()
            .contains(&PlanDiagnostic::ArtifactKindMismatch {
                transition: TransitionId::new("claim_code"),
                expected: ArtifactKindId::new("code"),
                actual: ArtifactKindId::new("implementation_pr"),
            })
    );
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

#[test]
fn artifact_scoped_state_legality_is_checked_before_planning() {
    let json = r#"{
        "name": "scoped-state-demo",
        "roles": [{"id": "architect", "queues": []}],
        "labels": [{"id": "design"}, {"id": "code"}, {"id": "ready"}],
        "artifact_kinds": [
            {"id": "design", "target": "issue", "identifying_labels": ["design"]},
            {"id": "code", "target": "issue", "identifying_labels": ["code"]}
        ],
        "state_dimensions": [{"id": "work_lifecycle", "states": [
            {"id": "ready", "label": "ready", "artifacts": ["code"]}
        ]}],
        "transitions": [{"id": "bad_ready", "artifact": "design", "roles": ["architect"],
            "effects": [{"kind": "add_label", "label": "ready"}]}]
    }"#;
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("json parses");
    let workflow = spec.validate().expect("workflow validates");
    let artifact = classify_issue(&workflow, 1, &["design"]);

    let error = workflow
        .planner()
        .plan_transition(
            &TransitionId::new("bad_ready"),
            &RoleId::new("architect"),
            &artifact,
        )
        .expect_err("ready is not legal for design in this workflow");

    assert!(
        error
            .diagnostics()
            .contains(&PlanDiagnostic::ImpossibleState {
                transition: TransitionId::new("bad_ready"),
                dimension: StateDimensionId::new("work_lifecycle"),
                states: vec![StateId::new("ready")],
            })
    );
}

#[test]
fn close_parent_issues_is_planned_as_workflow_effect() {
    let json = r#"{
        "name": "close-parents-test",
        "labels": [{"id": "code"}, {"id": "implementation"}],
        "artifact_kinds": [
            {"id": "code", "target": "issue", "identifying_labels": ["code"]},
            {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
        ],
        "roles": [{"id": "engineer"}],
        "transitions": [{
            "id": "land_pr",
            "artifact": "implementation_pr",
            "roles": ["engineer"],
            "effects": [
                {"kind": "merge_pull_request"},
                {"kind": "close_parent_issues"}
            ]
        }]
    }"#;
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("json parses");
    let workflow = spec.validate().expect("workflow validates");
    let artifact = classify_pr(&workflow, 10, &["implementation"]);
    let plan = workflow
        .planner()
        .plan_transition(
            &TransitionId::new("land_pr"),
            &RoleId::new("engineer"),
            &artifact,
        )
        .expect("engineer can land PR");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::MergePullRequest,
            WorkflowEffect::CloseParentIssues,
        ]
    );
    assert!(
        plan.postconditions.is_empty(),
        "close_parent_issues has no postcondition"
    );
}
