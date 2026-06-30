use super::*;

fn validator_spec() -> RawWorkflowSpec {
    let json = r#"{
        "name": "validator-bindings",
        "roles": [{"id": "validator"}],
        "labels": [{"id": "implementation-pr"}, {"id": "epic"}],
        "artifact_kinds": [
            {
                "id": "implementation_pr",
                "target": "pull_request",
                "identifying_labels": ["implementation-pr"]
            },
            {
                "id": "epic",
                "target": "issue",
                "identifying_labels": ["epic"]
            }
        ],
        "transitions": [
            {
                "id": "validate_merged_pr",
                "artifact": "implementation_pr",
                "roles": ["validator"]
            },
            {
                "id": "validate_epic",
                "artifact": "epic",
                "roles": ["validator"]
            }
        ],
        "validation_bindings": [
            {
                "id": "validate_each_merged_implementation_pr",
                "role": "validator",
                "action": "validate_merged_pr",
                "target_artifact": "implementation_pr",
                "trigger": {
                    "kind": "native_merge",
                    "branch": "main",
                    "artifact": "implementation_pr"
                },
                "readiness": {
                    "all": ["merged_to_default_branch", "ci_passed_at_merge"]
                },
                "target_selection": {"kind": "triggering_artifact"},
                "aggregation": {"include": ["source_issue", "produced_pr_relation"]},
                "idempotency_key": "validator:{binding_id}:pr:{pr_number}:main:{merged_main_sha}"
            },
            {
                "id": "validate_epic_when_ready",
                "role": "validator",
                "action": "validate_epic",
                "target_artifact": "epic",
                "trigger": {
                    "any": [
                        {"label_added": "validation-ready"},
                        {"child_completion_changed": true}
                    ]
                },
                "readiness": {
                    "any": [
                        {"labels": ["validation-ready"]},
                        {
                            "all_children": {
                                "issues": "closed_or_workflow_done",
                                "produced_prs": "merged_to_default_branch",
                                "dependencies": "complete",
                                "blocking_gates": "passed"
                            }
                        }
                    ]
                },
                "target_selection": {
                    "kind": "related_artifact",
                    "relation": "parent",
                    "artifact": "epic"
                },
                "aggregation": {
                    "include": ["child_issues", "produced_prs", "diffs", "ci", "scenario_evidence"],
                    "child_depth": 2
                },
                "idempotency_key": "validator:{binding_id}:epic:{issue_number}:state:{aggregate_fingerprint}"
            }
        ]
    }"#;

    serde_json::from_str(json).expect("validator binding spec parses")
}

#[test]
fn validation_bindings_parse_validate_and_compile() {
    let spec = validator_spec();
    assert_eq!(spec.validation_bindings.len(), 2);

    let workflow = spec.validate().expect("validator binding spec validates");
    let bindings = workflow.validation_bindings();
    assert_eq!(bindings.len(), 2);

    let per_pr = &bindings[0];
    assert_eq!(per_pr.id.as_str(), "validate_each_merged_implementation_pr");
    assert_eq!(per_pr.role.as_str(), "validator");
    assert_eq!(per_pr.action.as_str(), "validate_merged_pr");
    assert_eq!(per_pr.target_artifact.as_str(), "implementation_pr");
    assert_eq!(
        per_pr.idempotency_key,
        "validator:{binding_id}:pr:{pr_number}:main:{merged_main_sha}"
    );
    assert!(matches!(
        &per_pr.trigger,
        ValidationBindingDetail::Structured(value)
            if value["kind"] == "native_merge" && value["branch"] == "main"
    ));

    let aggregate = &bindings[1];
    assert_eq!(aggregate.id.as_str(), "validate_epic_when_ready");
    assert_eq!(aggregate.target_artifact.as_str(), "epic");
    assert!(matches!(
        &aggregate.aggregation,
        ValidationBindingDetail::Structured(value)
            if value["child_depth"] == 2
                && value["include"].as_array().is_some_and(|items| items.len() == 5)
    ));

    let compiled = compile(&workflow);
    assert_eq!(compiled.validation_bindings().len(), 2);
    assert_eq!(
        compiled.validation_bindings()[0].id.as_str(),
        per_pr.id.as_str()
    );
    assert_eq!(
        compiled.validation_bindings()[1].action.as_str(),
        aggregate.action.as_str()
    );
}

#[test]
fn validation_binding_reference_errors_are_diagnosed() {
    let mut spec = validator_spec();
    spec.validation_bindings[0].role = "ghost_role".to_string();
    spec.validation_bindings[0].action = "ghost_action".to_string();
    spec.validation_bindings[1].target_artifact = "ghost_artifact".to_string();
    spec.validation_bindings
        .push(spec.validation_bindings[0].clone());

    let errors = spec
        .validate()
        .expect_err("invalid validation binding references must fail");
    let diagnostics = errors.diagnostics();

    assert!(diagnostics.contains(&Diagnostic::DuplicateId {
        kind: SymbolKind::ValidationBinding,
        id: "validate_each_merged_implementation_pr".to_string(),
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::Role,
        id: "ghost_role".to_string(),
        site: ReferenceSite::ValidationBindingRole {
            binding: "validate_each_merged_implementation_pr".to_string(),
        },
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::Transition,
        id: "ghost_action".to_string(),
        site: ReferenceSite::ValidationBindingAction {
            binding: "validate_each_merged_implementation_pr".to_string(),
        },
    }));
    assert!(diagnostics.contains(&Diagnostic::UndeclaredReference {
        expected: SymbolKind::ArtifactKind,
        id: "ghost_artifact".to_string(),
        site: ReferenceSite::ValidationBindingTargetArtifact {
            binding: "validate_epic_when_ready".to_string(),
        },
    }));
}

#[test]
fn validation_binding_action_contract_matches_existing_role_action_rules() {
    let mut spec = validator_spec();
    spec.roles.push(RawRole {
        id: "observer".to_string(),
        charter: None,
        prompt: Default::default(),
        external_tools: Vec::new(),
        concurrency: None,
        queues: Vec::new(),
    });
    spec.validation_bindings[0].role = "observer".to_string();
    spec.validation_bindings[1].action = "validate_merged_pr".to_string();

    let errors = spec
        .validate()
        .expect_err("invalid validation binding action contracts must fail");
    let diagnostics = errors.diagnostics();

    assert!(
        diagnostics.contains(&Diagnostic::ValidationBindingActionUnauthorized {
            binding: "validate_each_merged_implementation_pr".to_string(),
            role: "observer".to_string(),
            action: "validate_merged_pr".to_string(),
        })
    );
    assert!(
        diagnostics.contains(&Diagnostic::ValidationBindingActionArtifactMismatch {
            binding: "validate_epic_when_ready".to_string(),
            action: "validate_merged_pr".to_string(),
            target_artifact: "epic".to_string(),
            action_artifact: "implementation_pr".to_string(),
        })
    );
}
