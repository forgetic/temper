use super::*;

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
    assert_eq!(workflow.transitions().len(), 35);
    assert_eq!(workflow.gates().len(), 3);
    // +1: implementation_pr -> implementation_pr dependency, for coordinated
    // serial landing (ADR 0023).
    assert_eq!(workflow.relations().len(), 6);
    assert!(
        workflow
            .transitions()
            .iter()
            .all(|transition| !transition.id.as_str().starts_with("record_ci_"))
    );
    assert!(
        !workflow
            .labels()
            .iter()
            .any(|label| label.as_str() == "merge-ready"
                || label.as_str() == "needs-merge"
                || label.as_str().starts_with("ci-")
                || label.as_str().starts_with("review-"))
    );
    assert!(
        workflow
            .labels()
            .iter()
            .any(|label| label.as_str() == "landing")
    );
    assert!(
        workflow
            .labels()
            .iter()
            .any(|label| label.as_str() == "merge-conflict")
    );

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
    assert!(
        architect_queue
            .artifacts
            .contains(&ArtifactKindId::new("code"))
    );
    assert!(
        architect_queue
            .artifacts
            .contains(&ArtifactKindId::new("implementation_pr"))
    );
    let human_queue = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "needs_human")
        .expect("needs_human queue is declared");
    assert!(
        human_queue
            .artifacts
            .contains(&ArtifactKindId::new("design"))
    );
    assert!(human_queue.artifacts.contains(&ArtifactKindId::new("code")));

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
    assert!(
        raw_intake
            .artifacts
            .contains(&ArtifactKindId::new("intake"))
    );
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
fn architect_work_transitions_self_assign_declaratively() {
    let workflow = fixture_workflow();
    let architect = RoleId::new("architect");
    let expected = [
        "triage_intake",
        "triage_intake_to_code",
        "triage_intake_to_design",
        "triage_intake_breakdown",
        "triage_to_code",
        "triage_to_blocked_code",
        "triage_to_design",
        "refine_design",
        // `mark_code_ready` is deliberately label-only: dependency reconciliation
        // applies it mechanically once prerequisites land.
        "reconcile_landed",
        "resolve_architect_request",
        "request_owner_input",
        "resolve_code_architect_request",
    ];

    for transition_id in expected {
        let transition = workflow
            .transitions()
            .iter()
            .find(|transition| transition.id.as_str() == transition_id)
            .unwrap_or_else(|| panic!("{transition_id} transition is declared"));
        assert!(
            transition
                .effects
                .iter()
                .any(|effect| effect == &Effect::SetAssignee(architect.clone())),
            "{transition_id} should assign the architect role"
        );
    }
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
    assert!(
        compiled
            .labels()
            .get(&LabelId::new("review-approved"))
            .is_none()
    );
    assert!(
        compiled
            .labels()
            .get(&LabelId::new("merge-ready"))
            .is_none()
    );
    assert!(
        compiled
            .labels()
            .get(&LabelId::new("needs-merge"))
            .is_none()
    );
    assert!(compiled.labels().get(&LabelId::new("landing")).is_some());
    assert!(
        compiled
            .labels()
            .get(&LabelId::new("merge-conflict"))
            .is_some()
    );

    let owner_alignment = compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "owner_alignment")
        .expect("owner_alignment queue is compiled");
    assert_eq!(owner_alignment.min_depth, Some(5));
    assert_eq!(owner_alignment.max_age, Some(Duration::days(7)));

    assert!(
        compiled
            .labels()
            .get(&LabelId::new("testing-passed"))
            .is_none()
    );
    assert!(
        compiled
            .labels()
            .get(&LabelId::new("testing-failed"))
            .is_none()
    );

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
    assert_eq!(
        open_pr.outcomes.get(&VerdictId::new("needs_human")),
        Some(&TransitionId::new("request_code_human_input")),
        "open_pr routes the needs_human verdict to the code-artifact human escalation transition"
    );
    for routed in ["request_code_architect_input", "request_code_human_input"] {
        let escalation = compiled
            .transitions()
            .iter()
            .find(|transition| transition.id.as_str() == routed)
            .unwrap_or_else(|| panic!("{routed} transition is compiled"));
        assert_eq!(
            escalation.artifact,
            ArtifactKindId::new("code"),
            "{routed} is legal on the open_pr artifact (code)"
        );
    }
}
