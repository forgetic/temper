use super::*;

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

    assert!(
        planner
            .dependency_unblocks(&blocked, &DependencyStatus::default())
            .is_empty()
    );
    let gated = planner
        .plan_transition(
            &TransitionId::new("mark_code_ready"),
            &RoleId::new("architect"),
            &blocked,
        )
        .expect_err("mark_code_ready is gated until dependencies land");
    assert!(
        gated
            .diagnostics()
            .contains(&PlanDiagnostic::GateNotSatisfied {
                transition: TransitionId::new("mark_code_ready"),
                gate: GateId::new("dependency_gate"),
            })
    );

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
