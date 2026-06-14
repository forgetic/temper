use super::*;

use temper_forge::CreateRepository;
use temper_runner::{RunnerConfig, WorkspaceCheckout};
use temper_workflow::{CompiledWorkflow, ExternalToolId, RawWorkflowSpec, RoleId};

fn repo_input() -> CreateRepository {
    CreateRepository {
        owner: "acme".to_string(),
        name: "service".to_string(),
        default_branch: "main".to_string(),
        description: None,
    }
}

/// Env returning a complete, push-disabled workspace configuration so the
/// provider binds in tests without touching a real remote.
fn workspace_env(key: &str) -> Option<String> {
    match key {
        "TEMPER_CODING_WORKSPACE_ROOT" => Some("/tmp/workspace".to_string()),
        "TEMPER_CODING_WORKSPACE_COMMAND" => Some("true".to_string()),
        "TEMPER_CODING_WORKSPACE_PUSH" => Some("0".to_string()),
        _ => None,
    }
}

fn compiled(json: &str) -> CompiledWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("workflow json parses");
    spec.validate().expect("workflow validates").compile()
}

fn tool(id: &str) -> ExternalToolId {
    ExternalToolId::new(id)
}

#[test]
fn binds_every_declared_workspace_id_with_its_checkout_capability() {
    // A compiled workflow declaring three distinct workspace ids across roles.
    let compiled = compiled(
        r#"{
            "name": "three-workspaces",
            "roles": [
                {
                    "id": "architect",
                    "external_tools": [{
                        "id": "triage_workspace",
                        "description": "Read intake and route a verdict.",
                        "required": false
                    }],
                    "queues": []
                },
                {
                    "id": "engineer",
                    "external_tools": [{
                        "id": "coding_workspace",
                        "description": "Edit and commit a PR head.",
                        "required": false
                    }],
                    "queues": []
                },
                {
                    "id": "reviewer",
                    "external_tools": [{
                        "id": "review_workspace",
                        "description": "Read the PR diff and CI, route a verdict.",
                        "required": false
                    }],
                    "queues": []
                }
            ]
        }"#,
    );
    let mut config = RunnerConfig::new(repo_input());

    let executors =
        configure_external_tool_executors_with_env(&compiled, &mut config, workspace_env)
            .expect("binding succeeds");

    // Each declared id binds an executable provider under its declaring role.
    let architect = RoleId::new("architect");
    let engineer = RoleId::new("engineer");
    let reviewer = RoleId::new("reviewer");
    assert!(
        executors
            .workspace_for(&architect, &tool("triage_workspace"))
            .is_some()
    );
    assert!(
        executors
            .workspace_for(&engineer, &tool("coding_workspace"))
            .is_some()
    );
    assert!(
        executors
            .workspace_for(&reviewer, &tool("review_workspace"))
            .is_some()
    );

    // The runner metadata binding matches the executable bindings: exactly the
    // three declared ids, each under its role.
    assert!(config.has_external_tool_binding(&architect, &tool("triage_workspace")));
    assert!(config.has_external_tool_binding(&engineer, &tool("coding_workspace")));
    assert!(config.has_external_tool_binding(&reviewer, &tool("review_workspace")));

    // The runtime validates: every executable provider is declared and bound.
    executors
        .validate(&compiled, &config)
        .expect("all bindings validate");

    // Each tool id gets the checkout capability appropriate to its work: the
    // engineer writes a head, the architect reads, the reviewer reads the PR.
    assert_eq!(
        executors.checkout_for(&engineer, &tool("coding_workspace")),
        Some(WorkspaceCheckout::Writable)
    );
    assert_eq!(
        executors.checkout_for(&architect, &tool("triage_workspace")),
        Some(WorkspaceCheckout::ReadOnly)
    );
    assert_eq!(
        executors.checkout_for(&reviewer, &tool("review_workspace")),
        Some(WorkspaceCheckout::PullRequestReadOnly)
    );
}

#[test]
fn reference_delivery_binds_all_three_operator_path_workspaces() {
    let compiled = temper_reference_delivery::workflow().compile();
    let mut config = RunnerConfig::new(repo_input());

    let executors =
        configure_external_tool_executors_with_env(&compiled, &mut config, workspace_env)
            .expect("binding succeeds");

    for (role, id, checkout) in [
        ("architect", "triage_workspace", WorkspaceCheckout::ReadOnly),
        ("engineer", "coding_workspace", WorkspaceCheckout::Writable),
        (
            "reviewer",
            "review_workspace",
            WorkspaceCheckout::PullRequestReadOnly,
        ),
    ] {
        let role = RoleId::new(role);
        assert!(
            executors.workspace_for(&role, &tool(id)).is_some(),
            "{id} should be bound for {role:?}"
        );
        assert_eq!(executors.checkout_for(&role, &tool(id)), Some(checkout));
    }

    executors
        .validate(&compiled, &config)
        .expect("reference-delivery bindings validate");
}

#[test]
fn non_workspace_external_tools_are_not_bound() {
    // A role declaring a non-workspace external tool: the local-git provider
    // leaves it to a different provider and never binds it to a git checkout.
    let compiled = compiled(
        r#"{
            "name": "mixed-tools",
            "roles": [{
                "id": "engineer",
                "external_tools": [
                    {
                        "id": "coding_workspace",
                        "description": "Edit and commit a PR head.",
                        "required": false
                    },
                    {
                        "id": "project_notes",
                        "description": "Read project notes.",
                        "required": false
                    }
                ],
                "queues": []
            }]
        }"#,
    );
    let mut config = RunnerConfig::new(repo_input());

    let executors =
        configure_external_tool_executors_with_env(&compiled, &mut config, workspace_env)
            .expect("binding succeeds");

    let engineer = RoleId::new("engineer");
    assert!(
        executors
            .workspace_for(&engineer, &tool("coding_workspace"))
            .is_some()
    );
    assert!(
        executors
            .workspace_for(&engineer, &tool("project_notes"))
            .is_none(),
        "a non-workspace tool must not bind a git workspace provider"
    );
    assert!(!config.has_external_tool_binding(&engineer, &tool("project_notes")));
}

#[test]
fn no_binding_when_no_role_declares_a_workspace() {
    // The diagnostic-path workflow: env is configured but no role declares any
    // workspace tool. Nothing binds, and the function still succeeds (the
    // eprintln diagnostic is preserved for the operator).
    let compiled = compiled(
        r#"{
            "name": "no-workspace",
            "roles": [{
                "id": "engineer",
                "external_tools": [{
                    "id": "project_notes",
                    "description": "Read project notes.",
                    "required": false
                }],
                "queues": []
            }]
        }"#,
    );
    let mut config = RunnerConfig::new(repo_input());

    let executors =
        configure_external_tool_executors_with_env(&compiled, &mut config, workspace_env)
            .expect("binding succeeds even with nothing to bind");

    let engineer = RoleId::new("engineer");
    assert!(
        executors
            .workspace_for(&engineer, &tool("project_notes"))
            .is_none()
    );
    assert!(!config.has_external_tool_binding(&engineer, &tool("project_notes")));
}

#[test]
fn no_binding_when_workspace_env_is_absent() {
    // Without workspace env, nothing binds even when a role declares a workspace.
    let compiled = compiled(
        r#"{
            "name": "declared-but-unbound",
            "roles": [{
                "id": "engineer",
                "external_tools": [{
                    "id": "coding_workspace",
                    "description": "Edit and commit a PR head.",
                    "required": false
                }],
                "queues": []
            }]
        }"#,
    );
    let mut config = RunnerConfig::new(repo_input());

    let executors = configure_external_tool_executors_with_env(&compiled, &mut config, |_| None)
        .expect("absent env is valid");

    let engineer = RoleId::new("engineer");
    assert!(
        executors
            .workspace_for(&engineer, &tool("coding_workspace"))
            .is_none()
    );
    assert!(!config.has_external_tool_binding(&engineer, &tool("coding_workspace")));
}
