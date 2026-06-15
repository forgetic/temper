use super::*;

#[test]
fn missing_state_artifact_reference_is_diagnosed() {
    let mut spec = valid_spec();
    spec.state_dimensions[0].states[0]
        .artifacts
        .push("ghost".to_string());

    let errors = spec.validate().expect_err("missing artifact must fail");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::UndeclaredReference {
                expected: SymbolKind::ArtifactKind,
                id: "ghost".to_string(),
                site: ReferenceSite::StateArtifact {
                    dimension: "code_lifecycle".to_string(),
                    state: "ready".to_string(),
                },
            })
    );
}

#[test]
fn missing_transition_role_reference_is_diagnosed() {
    let mut spec = valid_spec();
    spec.transitions[0].roles.push("ghost".to_string());

    let errors = spec.validate().expect_err("missing role must fail");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::UndeclaredReference {
                expected: SymbolKind::Role,
                id: "ghost".to_string(),
                site: ReferenceSite::TransitionRole {
                    transition: "claim_code".to_string(),
                },
            })
    );
}

#[test]
fn missing_assignee_effect_role_reference_is_diagnosed() {
    let mut spec = valid_spec();
    spec.transitions[0].effects.push(RawEffect::SetAssignee {
        role: "ghost".to_string(),
    });

    let errors = spec.validate().expect_err("missing effect role must fail");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::UndeclaredReference {
                expected: SymbolKind::Role,
                id: "ghost".to_string(),
                site: ReferenceSite::TransitionEffectRole {
                    transition: "claim_code".to_string(),
                },
            })
    );
}

#[test]
fn missing_relation_endpoint_references_are_diagnosed() {
    let mut spec = valid_spec();
    spec.relations.push(RawRelation {
        kind: RelationKind::Dependency,
        source: "ghost_source".to_string(),
        target: "ghost_target".to_string(),
    });

    let errors = spec
        .validate()
        .expect_err("missing relation endpoints fail");
    let diagnostics = errors.diagnostics();

    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::ArtifactKind,
        id: "ghost_source".to_string(),
        site: ReferenceSite::RelationSource {
            relation: "dependency ghost_source->ghost_target".to_string(),
        },
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::ArtifactKind,
        id: "ghost_target".to_string(),
        site: ReferenceSite::RelationTarget {
            relation: "dependency ghost_source->ghost_target".to_string(),
        },
    }));
}

#[test]
fn missing_queue_artifact_reference_is_diagnosed() {
    let mut spec = valid_spec();
    spec.queues[0].artifacts = vec!["nonexistent".to_string()];

    let errors = spec.validate().expect_err("missing artifact must fail");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::UndeclaredReference {
                expected: SymbolKind::ArtifactKind,
                id: "nonexistent".to_string(),
                site: ReferenceSite::QueueArtifact {
                    queue: "code_ready".to_string(),
                },
            })
    );
}

#[test]
fn missing_label_references_are_diagnosed_across_sites() {
    let mut spec = valid_spec();
    spec.artifact_kinds[1]
        .identifying_labels
        .push("a-missing".to_string());
    spec.queues[0].labels.push("q-missing".to_string());
    spec.queues[0].any_of.push(RawQueueLabelSet {
        labels: vec!["q-any-missing".to_string()],
    });
    spec.state_dimensions[0].states[0].label = Some("s-missing".to_string());
    spec.transitions[0].effects.push(RawEffect::AddLabel {
        label: "e-missing".to_string(),
    });
    spec.gates.push(RawGate {
        id: "label_gate".to_string(),
        satisfied_by: Vec::new(),
        condition: Some(RawGateCondition::LabelPresent {
            label: "g-missing".to_string(),
        }),
    });

    let errors = spec.validate().expect_err("missing labels must fail");
    let diagnostics = errors.diagnostics();

    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::Label,
        id: "a-missing".to_string(),
        site: ReferenceSite::ArtifactLabel {
            artifact: "code".to_string(),
        },
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::Label,
        id: "q-missing".to_string(),
        site: ReferenceSite::QueueLabel {
            queue: "code_ready".to_string(),
        },
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::Label,
        id: "q-any-missing".to_string(),
        site: ReferenceSite::QueueLabel {
            queue: "code_ready".to_string(),
        },
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::Label,
        id: "s-missing".to_string(),
        site: ReferenceSite::StateLabel {
            dimension: "code_lifecycle".to_string(),
            state: "ready".to_string(),
        },
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::Label,
        id: "e-missing".to_string(),
        site: ReferenceSite::TransitionEffectLabel {
            transition: "claim_code".to_string(),
        },
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::Label,
        id: "g-missing".to_string(),
        site: ReferenceSite::GateCondition {
            gate: "label_gate".to_string(),
        },
    }));
}

#[test]
fn missing_gate_and_transition_references_are_diagnosed() {
    let mut spec = valid_spec();
    spec.transitions[0]
        .requires_gates
        .push("absent_gate".to_string());
    spec.gates[0]
        .satisfied_by
        .push("absent_transition".to_string());
    // Also unhook a role queue reference to confirm role -> queue checks.
    spec.roles[0].queues.push("absent_queue".to_string());

    let errors = spec.validate().expect_err("missing refs must fail");
    let diagnostics = errors.diagnostics();

    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::Gate,
        id: "absent_gate".to_string(),
        site: ReferenceSite::TransitionGate {
            transition: "claim_code".to_string(),
        },
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::Transition,
        id: "absent_transition".to_string(),
        site: ReferenceSite::GateTransition {
            gate: "review_gate".to_string(),
        },
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::Queue,
        id: "absent_queue".to_string(),
        site: ReferenceSite::RoleQueue {
            role: "engineer".to_string(),
        },
    }));
}

#[test]
fn missing_gate_state_condition_references_are_diagnosed() {
    let mut spec = valid_spec();
    spec.gates.push(RawGate {
        id: "state_gate".to_string(),
        satisfied_by: Vec::new(),
        condition: Some(RawGateCondition::StateEquals {
            dimension: "code_lifecycle".to_string(),
            state: "absent_state".to_string(),
        }),
    });
    spec.gates.push(RawGate {
        id: "dimension_gate".to_string(),
        satisfied_by: Vec::new(),
        condition: Some(RawGateCondition::StateEquals {
            dimension: "absent_dimension".to_string(),
            state: "ready".to_string(),
        }),
    });

    let errors = spec
        .validate()
        .expect_err("missing condition refs must fail");
    let diagnostics = errors.diagnostics();

    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::State,
        id: "absent_state".to_string(),
        site: ReferenceSite::GateCondition {
            gate: "state_gate".to_string(),
        },
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::StateDimension,
        id: "absent_dimension".to_string(),
        site: ReferenceSite::GateCondition {
            gate: "dimension_gate".to_string(),
        },
    }));
}

#[test]
fn validation_collects_multiple_diagnostics_at_once() {
    let mut spec = valid_spec();
    spec.roles.push(RawRole {
        id: "engineer".to_string(),
        charter: None,
        prompt: Default::default(),
        external_tools: Vec::new(),
        concurrency: None,
        queues: vec!["absent_queue".to_string()],
    });
    spec.transitions[0].roles.push("ghost".to_string());

    let errors = spec.validate().expect_err("multiple problems must fail");
    // At least: duplicate role, missing queue, missing transition role.
    assert!(
        errors.len() >= 3,
        "expected several diagnostics, got {}",
        errors.len()
    );
}

#[test]
fn undeclared_initial_label_is_diagnosed_like_an_artifact_label() {
    let mut spec = valid_spec();
    spec.artifact_kinds[1]
        .initial_labels
        .push("missing-initial".to_string());

    let errors = spec
        .validate()
        .expect_err("missing initial label must fail");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::UndeclaredReference {
                expected: SymbolKind::Label,
                id: "missing-initial".to_string(),
                site: ReferenceSite::ArtifactLabel {
                    artifact: "code".to_string(),
                },
            })
    );
}
