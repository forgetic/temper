use std::sync::Arc;

use async_trait::async_trait;
use temper_forge::CreateRepository;
use temper_runner::{
    CodingWorkspace, CodingWorkspaceError, CodingWorkspaceOutput, CodingWorkspaceRequest,
    ExternalToolBindingError, ExternalToolExecutors, RunnerConfig,
};
use temper_workflow::{CompiledWorkflow, ExternalToolId, RawWorkflowSpec, RoleId};

fn repo_input() -> CreateRepository {
    CreateRepository {
        owner: "acme".to_string(),
        name: "service".to_string(),
        default_branch: "main".to_string(),
        description: None,
    }
}

fn compiled_workflow(coding_workspace_required: bool) -> CompiledWorkflow {
    let required = if coding_workspace_required {
        "true"
    } else {
        "false"
    };
    let json = format!(
        r#"{{
            "name": "external-tool-runner-test",
            "roles": [{{
                "id": "banana",
                "external_tools": [
                    {{
                        "id": "coding_workspace",
                        "description": "Edit and commit repository code.",
                        "required": {required},
                        "constraints": ["Only edit the checked-out repository."],
                        "guidance": "Use before opening implementation PRs."
                    }},
                    {{
                        "id": "project_notes",
                        "description": "Read project notes.",
                        "required": false
                    }}
                ],
                "queues": []
            }}]
        }}"#,
    );
    let spec: RawWorkflowSpec = serde_json::from_str(&json).expect("workflow json parses");
    spec.validate().expect("workflow validates").compile()
}

fn role_id() -> RoleId {
    RoleId::new("banana")
}

fn tool_id(id: &str) -> ExternalToolId {
    ExternalToolId::new(id)
}

struct DummyWorkspace;

#[async_trait]
impl CodingWorkspace for DummyWorkspace {
    async fn produce_head(
        &self,
        _request: CodingWorkspaceRequest,
    ) -> Result<CodingWorkspaceOutput, CodingWorkspaceError> {
        Err(CodingWorkspaceError::new("not used in runner config tests"))
    }
}

#[test]
fn required_unbound_external_tool_fails_preflight_with_clear_error() {
    let compiled = compiled_workflow(true);
    let config = RunnerConfig::new(repo_input());

    let error = config
        .validate_external_tool_bindings(&compiled)
        .expect_err("required external tool needs a binding");

    assert_eq!(
        error,
        ExternalToolBindingError::MissingRequired {
            role: role_id(),
            tool: tool_id("coding_workspace"),
        }
    );
    assert!(
        error
            .to_string()
            .contains("role `banana` requires external tool `coding_workspace`")
    );
}

#[test]
fn optional_unbound_external_tools_are_omitted() {
    let compiled = compiled_workflow(false);
    let role = compiled.role(&role_id()).expect("role exists");
    let config = RunnerConfig::new(repo_input());

    config
        .validate_external_tool_bindings(&compiled)
        .expect("optional unbound tools are allowed");
    let bound = config
        .bound_external_tools_for(role)
        .expect("optional unbound tools do not error");

    assert!(bound.is_empty());
}

#[test]
fn bound_declared_external_tool_surfaces_manifest_metadata() {
    let compiled = compiled_workflow(true);
    let role = compiled.role(&role_id()).expect("role exists");
    let config = RunnerConfig::new(repo_input()).with_external_tool_binding(
        role_id(),
        tool_id("coding_workspace"),
        "workspace-local",
    );

    config
        .validate_external_tool_bindings(&compiled)
        .expect("declared binding validates");
    let bound = config
        .bound_external_tools_for(role)
        .expect("declared binding resolves");

    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0].id, "coding_workspace");
    assert_eq!(bound[0].provider, "workspace-local");
    assert_eq!(bound[0].description, "Edit and commit repository code.");
    assert_eq!(
        bound[0].constraints,
        vec!["Only edit the checked-out repository.".to_string()]
    );
    assert_eq!(
        bound[0].guidance.as_deref(),
        Some("Use before opening implementation PRs.")
    );
}

#[test]
fn undeclared_external_tool_binding_does_not_grant_authority() {
    let compiled = compiled_workflow(false);
    let config = RunnerConfig::new(repo_input()).with_external_tool_binding(
        role_id(),
        tool_id("shell"),
        "shell-provider",
    );

    let error = config
        .validate_external_tool_bindings(&compiled)
        .expect_err("undeclared binding must fail");

    assert_eq!(
        error,
        ExternalToolBindingError::UndeclaredTool {
            role: role_id(),
            tool: tool_id("shell"),
        }
    );
}

#[test]
fn external_tool_binding_for_unknown_role_fails() {
    let compiled = compiled_workflow(false);
    let config = RunnerConfig::new(repo_input()).with_external_tool_binding(
        RoleId::new("ghost"),
        tool_id("coding_workspace"),
        "workspace-local",
    );

    let error = config
        .validate_external_tool_bindings(&compiled)
        .expect_err("unknown role binding must fail");

    assert_eq!(
        error,
        ExternalToolBindingError::UnknownRole {
            role: RoleId::new("ghost"),
        }
    );
}

#[test]
fn executable_tool_provider_requires_runner_metadata_binding() {
    let compiled = compiled_workflow(false);
    let config = RunnerConfig::new(repo_input());
    let executors = ExternalToolExecutors::new().with_workspace(
        role_id(),
        tool_id("coding_workspace"),
        Arc::new(DummyWorkspace),
    );

    let error = executors
        .validate(&compiled, &config)
        .expect_err("executable provider without binding must fail");

    assert_eq!(
        error,
        ExternalToolBindingError::ExecutableWithoutBinding {
            role: role_id(),
            tool: tool_id("coding_workspace"),
        }
    );
}

#[test]
fn executable_tool_provider_validates_when_declared_and_bound() {
    let compiled = compiled_workflow(false);
    let config = RunnerConfig::new(repo_input()).with_external_tool_binding(
        role_id(),
        tool_id("coding_workspace"),
        "workspace-local",
    );
    let executors = ExternalToolExecutors::new().with_workspace(
        role_id(),
        tool_id("coding_workspace"),
        Arc::new(DummyWorkspace),
    );

    executors
        .validate(&compiled, &config)
        .expect("declared, bound executable validates");
}
