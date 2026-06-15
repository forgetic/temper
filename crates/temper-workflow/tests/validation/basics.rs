use super::*;

#[test]
fn minimal_valid_workflow_validates() {
    let spec = valid_spec();
    let workflow: ValidatedWorkflow = spec.validate().expect("spec should validate");

    assert_eq!(workflow.name(), "code-review");
    assert_eq!(workflow.roles().len(), 2);
    assert_eq!(workflow.labels().len(), 6);
    assert_eq!(workflow.artifact_kinds().len(), 2);
    assert_eq!(workflow.relations().len(), 1);
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
        prompt: Default::default(),
        external_tools: Vec::new(),
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
    assert!(
        errors
            .diagnostics()
            .iter()
            .all(|d| d.severity() == Severity::Error)
    );
}

#[test]
fn duplicate_state_id_is_diagnosed_per_dimension() {
    let mut spec = valid_spec();
    spec.state_dimensions[0].states.push(RawState {
        id: "ready".to_string(),
        label: None,
        artifacts: Vec::new(),
    });

    let errors = spec.validate().expect_err("duplicate state must fail");
    assert!(errors.diagnostics().contains(&Diagnostic::DuplicateState {
        dimension: "code_lifecycle".to_string(),
        id: "ready".to_string(),
    }));
}

#[test]
fn raw_spec_loads_from_json() {
    let json = r#"{
        "name": "from-json",
        "labels": [{"id": "ready"}, {"id": "needs-review"}],
        "artifact_kinds": [{
            "id": "code",
            "target": "issue",
            "identifying_labels": ["ready"],
            "initial_labels": ["needs-review"]
        }],
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
    assert_eq!(spec.artifact_kinds[0].initial_labels, vec!["needs-review"]);

    let workflow = spec.validate().expect("loaded spec should validate");
    assert_eq!(workflow.name(), "from-json");
    assert_eq!(workflow.transitions().len(), 1);
    assert_eq!(
        workflow.artifact_kinds()[0].initial_labels,
        vec!["needs-review".into()]
    );
}

#[test]
fn raw_artifact_kind_initial_labels_default_to_empty() {
    let json = r#"{
        "id": "code",
        "target": "issue",
        "identifying_labels": ["code"]
    }"#;

    let artifact: RawArtifactKind = serde_json::from_str(json).expect("artifact parses");
    assert!(artifact.initial_labels.is_empty());
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
