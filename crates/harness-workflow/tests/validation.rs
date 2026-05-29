//! Validation tests for `harness-workflow` Phase 2 (spec + validation).

use harness_workflow::{
    ArtifactTarget, Diagnostic, RawArtifactKind, RawEffect, RawGate, RawGateCondition, RawLabel,
    RawQueue, RawRole, RawState, RawStateDimension, RawTransition, RawWorkflowSpec, ReferenceSite,
    Severity, SymbolKind, ValidatedWorkflow,
};

/// Builds a small but complete workflow that exercises every reference kind:
/// role -> queue, queue -> artifact, queue -> label, artifact -> label,
/// state -> label, transition -> artifact/role/gate/effect-label, and
/// gate -> transition/condition.
fn valid_spec() -> RawWorkflowSpec {
    RawWorkflowSpec {
        name: "code-review".to_string(),
        labels: vec![
            RawLabel {
                id: "code".to_string(),
                description: Some("identifies a code artifact".to_string()),
            },
            RawLabel {
                id: "ready".to_string(),
                description: Some("ready to claim".to_string()),
            },
            RawLabel {
                id: "in-progress".to_string(),
                description: None,
            },
            RawLabel {
                id: "needs-review".to_string(),
                description: None,
            },
            RawLabel {
                id: "review-approved".to_string(),
                description: None,
            },
        ],
        roles: vec![
            RawRole {
                id: "engineer".to_string(),
                charter: Some("implements code issues".to_string()),
                concurrency: Some(2),
                queues: vec!["code_ready".to_string()],
            },
            RawRole {
                id: "reviewer".to_string(),
                charter: None,
                concurrency: None,
                queues: vec!["needs_review".to_string()],
            },
        ],
        artifact_kinds: vec![RawArtifactKind {
            id: "code".to_string(),
            target: ArtifactTarget::Issue,
            identifying_labels: vec!["code".to_string()],
        }],
        state_dimensions: vec![RawStateDimension {
            id: "code_lifecycle".to_string(),
            exclusive: true,
            states: vec![
                RawState {
                    id: "ready".to_string(),
                    label: Some("ready".to_string()),
                },
                RawState {
                    id: "in_progress".to_string(),
                    label: Some("in-progress".to_string()),
                },
            ],
        }],
        queues: vec![
            RawQueue {
                id: "code_ready".to_string(),
                artifact: "code".to_string(),
                labels: vec!["ready".to_string()],
            },
            RawQueue {
                id: "needs_review".to_string(),
                artifact: "code".to_string(),
                labels: vec!["needs-review".to_string()],
            },
        ],
        transitions: vec![
            RawTransition {
                id: "claim_code".to_string(),
                artifact: "code".to_string(),
                roles: vec!["engineer".to_string()],
                requires_gates: Vec::new(),
                effects: vec![
                    RawEffect::RemoveLabel {
                        label: "ready".to_string(),
                    },
                    RawEffect::AddLabel {
                        label: "in-progress".to_string(),
                    },
                ],
            },
            RawTransition {
                id: "approve_review".to_string(),
                artifact: "code".to_string(),
                roles: vec!["reviewer".to_string()],
                requires_gates: vec!["review_gate".to_string()],
                effects: vec![RawEffect::AddLabel {
                    label: "review-approved".to_string(),
                }],
            },
        ],
        gates: vec![RawGate {
            id: "review_gate".to_string(),
            satisfied_by: vec!["approve_review".to_string()],
            condition: None,
        }],
    }
}

#[test]
fn minimal_valid_workflow_validates() {
    let spec = valid_spec();
    let workflow: ValidatedWorkflow = spec.validate().expect("spec should validate");

    assert_eq!(workflow.name(), "code-review");
    assert_eq!(workflow.roles().len(), 2);
    assert_eq!(workflow.labels().len(), 5);
    assert_eq!(workflow.artifact_kinds().len(), 1);
    assert_eq!(workflow.state_dimensions().len(), 1);
    assert_eq!(workflow.queues().len(), 2);
    assert_eq!(workflow.transitions().len(), 2);
    assert_eq!(workflow.gates().len(), 1);
}

#[test]
fn empty_named_workflow_validates() {
    let spec = RawWorkflowSpec {
        name: "empty".to_string(),
        ..RawWorkflowSpec::default()
    };
    let workflow = spec.validate().expect("empty workflow is valid");
    assert_eq!(workflow.name(), "empty");
    assert!(workflow.roles().is_empty());
}

#[test]
fn duplicate_role_id_is_diagnosed() {
    let mut spec = valid_spec();
    spec.roles.push(RawRole {
        id: "engineer".to_string(),
        charter: None,
        concurrency: None,
        queues: Vec::new(),
    });

    let errors = spec.validate().expect_err("duplicate role must fail");
    assert!(errors.diagnostics().contains(&Diagnostic::DuplicateId {
        kind: SymbolKind::Role,
        id: "engineer".to_string(),
    }));
}

#[test]
fn duplicate_label_id_is_diagnosed() {
    let mut spec = valid_spec();
    spec.labels.push(RawLabel {
        id: "ready".to_string(),
        description: None,
    });

    let errors = spec.validate().expect_err("duplicate label must fail");
    assert!(errors.diagnostics().contains(&Diagnostic::DuplicateId {
        kind: SymbolKind::Label,
        id: "ready".to_string(),
    }));
    assert!(errors
        .diagnostics()
        .iter()
        .all(|d| d.severity() == Severity::Error));
}

#[test]
fn duplicate_state_id_is_diagnosed_per_dimension() {
    let mut spec = valid_spec();
    spec.state_dimensions[0].states.push(RawState {
        id: "ready".to_string(),
        label: None,
    });

    let errors = spec.validate().expect_err("duplicate state must fail");
    assert!(errors.diagnostics().contains(&Diagnostic::DuplicateState {
        dimension: "code_lifecycle".to_string(),
        id: "ready".to_string(),
    }));
}

#[test]
fn missing_transition_role_reference_is_diagnosed() {
    let mut spec = valid_spec();
    spec.transitions[0].roles.push("ghost".to_string());

    let errors = spec.validate().expect_err("missing role must fail");
    assert!(errors
        .diagnostics()
        .contains(&Diagnostic::UndeclaredReference {
            expected: SymbolKind::Role,
            id: "ghost".to_string(),
            site: ReferenceSite::TransitionRole {
                transition: "claim_code".to_string(),
            },
        }));
}

#[test]
fn missing_assignee_effect_role_reference_is_diagnosed() {
    let mut spec = valid_spec();
    spec.transitions[0].effects.push(RawEffect::SetAssignee {
        role: "ghost".to_string(),
    });

    let errors = spec.validate().expect_err("missing effect role must fail");
    assert!(errors
        .diagnostics()
        .contains(&Diagnostic::UndeclaredReference {
            expected: SymbolKind::Role,
            id: "ghost".to_string(),
            site: ReferenceSite::TransitionEffectRole {
                transition: "claim_code".to_string(),
            },
        }));
}

#[test]
fn missing_queue_artifact_reference_is_diagnosed() {
    let mut spec = valid_spec();
    spec.queues[0].artifact = "nonexistent".to_string();

    let errors = spec.validate().expect_err("missing artifact must fail");
    assert!(errors
        .diagnostics()
        .contains(&Diagnostic::UndeclaredReference {
            expected: SymbolKind::ArtifactKind,
            id: "nonexistent".to_string(),
            site: ReferenceSite::QueueArtifact {
                queue: "code_ready".to_string(),
            },
        }));
}

#[test]
fn missing_label_references_are_diagnosed_across_sites() {
    let mut spec = valid_spec();
    spec.artifact_kinds[0]
        .identifying_labels
        .push("a-missing".to_string());
    spec.queues[0].labels.push("q-missing".to_string());
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
fn raw_spec_loads_from_json() {
    let json = r#"{
        "name": "from-json",
        "labels": [{"id": "ready"}],
        "artifact_kinds": [{"id": "code", "target": "issue", "identifying_labels": ["ready"]}],
        "queues": [{"id": "code_ready", "artifact": "code", "labels": ["ready"]}],
        "roles": [{"id": "engineer", "queues": ["code_ready"]}],
        "transitions": [{
            "id": "claim",
            "artifact": "code",
            "roles": ["engineer"],
            "effects": [{"kind": "remove_label", "label": "ready"}]
        }]
    }"#;

    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("json should parse");
    let workflow = spec.validate().expect("loaded spec should validate");
    assert_eq!(workflow.name(), "from-json");
    assert_eq!(workflow.transitions().len(), 1);
}

#[test]
fn compiler_style_apis_require_validated_workflow() {
    // This stands in for the compiler/runtime APIs that later phases add: they
    // accept a `ValidatedWorkflow`, never a `RawWorkflowSpec`. The only way to
    // obtain a `ValidatedWorkflow` is through validation, so reaching this
    // function proves the workflow was validated first.
    fn compile_role_count(workflow: &ValidatedWorkflow) -> usize {
        workflow.roles().len()
    }

    let spec = valid_spec();
    let workflow = spec.validate().expect("spec should validate");
    assert_eq!(compile_role_count(&workflow), 2);
}
