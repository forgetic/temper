use harness_workflow::{Diagnostic, ExternalToolId, RawWorkflowSpec, RoleId};

fn spec_json() -> &'static str {
    r#"{
        "name": "external-tool-test",
        "roles": [{
            "id": "engineer",
            "prompt": {"tool_guidance": "Use declared tools only."},
            "external_tools": [
                {
                    "id": "coding_workspace",
                    "description": "Edit and commit repository code.",
                    "required": true,
                    "constraints": ["Only edit the checked-out repository."],
                    "guidance": "Use before opening implementation PRs."
                },
                {
                    "id": "project_notes",
                    "description": "Read project-specific notes."
                }
            ],
            "queues": ["todo"]
        }],
        "labels": [{"id": "task"}, {"id": "todo"}, {"id": "done"}],
        "artifact_kinds": [{
            "id": "task",
            "target": "issue",
            "identifying_labels": ["task"]
        }],
        "queues": [{"id": "todo", "artifact": "task", "labels": ["todo"]}],
        "transitions": [{
            "id": "advance",
            "artifact": "task",
            "roles": ["engineer"],
            "effects": [
                {"kind": "remove_label", "label": "todo"},
                {"kind": "add_label", "label": "done"}
            ]
        }]
    }"#
}

#[test]
fn external_tool_declarations_validate_and_compile_deterministically() {
    let spec: RawWorkflowSpec = serde_json::from_str(spec_json()).expect("spec parses");
    let workflow = spec.validate().expect("spec validates");
    let first = workflow.compile();
    let second = workflow.compile();
    assert_eq!(first, second);

    let role = first.role(&RoleId::new("engineer")).expect("role compiles");
    assert_eq!(role.external_tools.len(), 2);
    assert_eq!(
        role.external_tools[0].id,
        ExternalToolId::new("coding_workspace")
    );
    assert!(role.external_tools[0].required);
    assert_eq!(
        role.external_tools[1].id,
        ExternalToolId::new("project_notes")
    );
    assert!(!role.external_tools[1].required);

    let prompt = role.prompt.render();
    let actions = prompt.find("## Authorized workflow actions").unwrap();
    let external = prompt.find("## User-declared external tools").unwrap();
    assert!(actions < external);
    assert!(prompt.contains("coding_workspace: required - Edit and commit repository code."));
    assert!(prompt.contains("project_notes: optional - Read project-specific notes."));
    assert!(prompt.contains("A declaration is not executable unless the runner binds"));
}

#[test]
fn unknown_external_tool_fields_are_rejected_by_serde() {
    let json = spec_json().replace(
        "\"guidance\": \"Use before opening implementation PRs.\"",
        "\"guidance\": \"Use before opening implementation PRs.\", \"surprise\": true",
    );
    let error = serde_json::from_str::<RawWorkflowSpec>(&json).unwrap_err();
    assert!(error.to_string().contains("unknown field `surprise`"));
}

#[test]
fn duplicate_external_tool_ids_are_validation_errors_with_role_context() {
    let json = spec_json().replace("\"id\": \"project_notes\"", "\"id\": \"coding_workspace\"");
    let spec: RawWorkflowSpec = serde_json::from_str(&json).expect("spec parses");
    let error = spec.validate().expect_err("duplicate tool id is rejected");

    assert!(matches!(
        &error.diagnostics()[0],
        Diagnostic::DuplicateRoleExternalTool { role, id }
            if role == "engineer" && id == "coding_workspace"
    ));
    assert!(error.to_string().contains("duplicate external tool id"));
}
