use super::*;

#[test]
fn intake_triage_is_a_normal_queue_match() {
    let workflow = fixture_workflow();
    let planner = workflow.planner();
    let intake = classify_issue(&workflow, 1, &["untriaged"]);

    assert!(
        planner
            .matching_queues(&intake)
            .contains(&temper_workflow::QueueId::new("design_triage"))
    );

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
    assert!(
        planner
            .matching_queues(&architect_issue)
            .contains(&needs_architect)
    );
    assert!(
        planner
            .matching_queues(&architect_pr)
            .contains(&needs_architect)
    );

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
    assert!(
        planner
            .matching_queues(&owner_design)
            .contains(&needs_owner_queue)
    );
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
    assert!(
        planner
            .matching_queues(&human_design)
            .contains(&needs_human_queue)
    );
    let clear_human = TransitionId::new("clear_human_flag");
    let clear = planner
        .plan_transition(&clear_human, &human, &human_design)
        .unwrap();
    assert_eq!(
        clear.effects,
        vec![WorkflowEffect::RemoveLabel(needs_human)]
    );
}
