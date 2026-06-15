use super::*;

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
    assert!(
        blocked
            .diagnostics()
            .contains(&PlanDiagnostic::GateNotSatisfied {
                transition: TransitionId::new("land_pr"),
                gate: GateId::new("ci_gate"),
            })
    );

    let ci_only = GateSignals::new().with_ci(CiStatus::passed());
    let blocked = planner
        .plan_transition_with(
            &TransitionId::new("land_pr"),
            &RoleId::new("mechanical"),
            &ready,
            &ci_only,
        )
        .expect_err("a PR with landing and green CI still needs native approval");
    assert!(
        blocked
            .diagnostics()
            .contains(&PlanDiagnostic::GateNotSatisfied {
                transition: TransitionId::new("land_pr"),
                gate: GateId::new("review_gate"),
            })
    );

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

    assert!(
        planner
            .matching_queues(&fresh_pr)
            .contains(&QueueId::new("pr_needs_review"))
    );
}

#[test]
fn failed_gates_route_back_to_engineer_queues() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();

    let changes = classify_pr(&workflow, 20, &["implementation"]);
    let review_signal = GateSignals::new().with_review(ReviewStatus::new(false, true));
    assert!(
        planner
            .matching_queues_with(&changes, &review_signal)
            .contains(&QueueId::new("pr_changes_requested"))
    );

    let ci_signal = GateSignals::new().with_ci(CiStatus::failed());
    let failed = classify_pr(&workflow, 21, &["implementation"]);
    assert!(
        planner
            .matching_queues_with(&failed, &ci_signal)
            .contains(&QueueId::new("pr_ci_failed"))
    );

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
    assert!(
        planner
            .matching_queues(&conflicted)
            .contains(&QueueId::new("pr_merge_conflict"))
    );
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
