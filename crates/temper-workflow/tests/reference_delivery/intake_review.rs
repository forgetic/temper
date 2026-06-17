use super::*;

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
    assert!(
        planner
            .matching_queues(&stamped)
            .contains(&QueueId::new("design_triage"))
    );
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
            WorkflowEffect::SetAssignee {
                role: architect.clone(),
            },
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
            WorkflowEffect::SetAssignee {
                role: architect.clone(),
            },
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
            WorkflowEffect::SetAssignee { role: architect },
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
