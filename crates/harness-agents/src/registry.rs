//! Building [`AgentRegistry`] values for real, LLM-backed workflow roles.
//!
//! The production path is manifest-driven: given a compiled workflow, register
//! one generic [`LlmRoleAgent`](crate::LlmRoleAgent) for each compiled role. Role
//! ids and prompts therefore come from user workflow configuration rather than a
//! hard-coded reference-delivery list.
//!
use std::sync::Arc;

use harness_forge::Forge;
use harness_runner::{
    Agent, AgentRegistry, BoundExternalTool, ExternalToolBindingError, ExternalToolExecutors,
    RunnerConfig,
};
use harness_workflow::{CompiledWorkflow, RoleManifest, ValidatedWorkflow};

use crate::provider::ProviderConfig;
use crate::role::LlmRoleAgent;

/// Builds the production registry from compiled workflow role manifests.
///
/// Every role in `compiled.roles()` gets one generic LLM agent. No role ids,
/// prompt constants, or reference workflow behavior are baked into this builder.
/// Required external-tool declarations fail unless callers use
/// [`real_registry_from_compiled_with_external_tools`] with matching bindings.
pub fn real_registry_from_compiled<F>(
    provider: ProviderConfig,
    compiled: &CompiledWorkflow,
) -> Result<AgentRegistry<F>, ExternalToolBindingError>
where
    F: Forge + ?Sized + 'static,
{
    register_compiled_roles(provider, compiled, no_bound_external_tools)
}

/// Builds the production registry with runner-bound external tool metadata.
///
/// Required external tool declarations fail before agents are registered unless
/// `config` binds a matching provider. Optional unbound declarations are omitted
/// from each agent's runtime prompt/context.
pub fn real_registry_from_compiled_with_external_tools<F>(
    provider: ProviderConfig,
    compiled: &CompiledWorkflow,
    config: &RunnerConfig,
) -> Result<AgentRegistry<F>, ExternalToolBindingError>
where
    F: Forge + ?Sized + 'static,
{
    real_registry_from_compiled_with_external_tool_executors(
        provider,
        compiled,
        config,
        ExternalToolExecutors::new(),
    )
}

/// Builds the production registry with runner-bound external tool metadata and
/// executable provider objects such as coding workspaces.
pub fn real_registry_from_compiled_with_external_tool_executors<F>(
    provider: ProviderConfig,
    compiled: &CompiledWorkflow,
    config: &RunnerConfig,
    executors: ExternalToolExecutors,
) -> Result<AgentRegistry<F>, ExternalToolBindingError>
where
    F: Forge + ?Sized + 'static,
{
    config.validate_external_tool_bindings(compiled)?;
    executors.validate(compiled, config)?;
    register_compiled_roles(provider, compiled, |role| {
        Ok((config.bound_external_tools_for(role)?, executors.clone()))
    })
}

fn register_compiled_roles<F>(
    provider: ProviderConfig,
    compiled: &CompiledWorkflow,
    bound_tools_for: impl Fn(
        &RoleManifest,
    ) -> Result<
        (Vec<BoundExternalTool>, ExternalToolExecutors),
        ExternalToolBindingError,
    >,
) -> Result<AgentRegistry<F>, ExternalToolBindingError>
where
    F: Forge + ?Sized + 'static,
{
    let mut registry = AgentRegistry::new();
    for role in compiled.roles() {
        let (bound_tools, executors) = bound_tools_for(role)?;
        registry.insert(
            role.id.clone(),
            Arc::new(LlmRoleAgent::with_bound_external_tools_and_executors(
                role.clone(),
                provider.clone(),
                bound_tools,
                executors,
            )) as Arc<dyn Agent<F>>,
        );
    }
    Ok(registry)
}

fn no_bound_external_tools(
    role: &RoleManifest,
) -> Result<(Vec<BoundExternalTool>, ExternalToolExecutors), ExternalToolBindingError> {
    if let Some(tool) = role.external_tools.iter().find(|tool| tool.required) {
        return Err(ExternalToolBindingError::MissingRequired {
            role: role.id.clone(),
            tool: tool.id.clone(),
        });
    }
    Ok((Vec::new(), ExternalToolExecutors::new()))
}

/// Validates the common "compile once, then register roles" production shape.
///
/// This convenience exists for callers that still hold the type-phase workflow;
/// callers that already compiled should prefer [`real_registry_from_compiled`].
pub fn real_registry_from_workflow<F>(
    provider: ProviderConfig,
    workflow: &ValidatedWorkflow,
) -> Result<AgentRegistry<F>, ExternalToolBindingError>
where
    F: Forge + ?Sized + 'static,
{
    let compiled = workflow.compile();
    real_registry_from_compiled(provider, &compiled)
}

#[cfg(test)]
mod tests {
    use super::*;

    use harness_workflow::{RawWorkflowSpec, RoleId};

    fn provider() -> ProviderConfig {
        ProviderConfig::new("test-provider", "test-model", "http://127.0.0.1", "secret")
    }

    fn workflow() -> ValidatedWorkflow {
        parse_workflow(
            r#"{
                "name": "synthetic-user-roles",
                "roles": [
                    {"id": "banana", "queues": ["todo"]},
                    {"id": "kumquat", "queues": []}
                ],
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
                    "roles": ["banana"],
                    "effects": [
                        {"kind": "remove_label", "label": "todo"},
                        {"kind": "add_label", "label": "done"}
                    ]
                }]
            }"#,
        )
    }

    fn required_tool_workflow() -> ValidatedWorkflow {
        parse_workflow(
            r#"{
                "name": "synthetic-user-roles",
                "roles": [{
                    "id": "banana",
                    "external_tools": [{
                        "id": "coding_workspace",
                        "description": "Edit and commit repository code.",
                        "required": true
                    }],
                    "queues": []
                }]
            }"#,
        )
    }

    fn parse_workflow(json: &str) -> ValidatedWorkflow {
        let spec: RawWorkflowSpec = serde_json::from_str(json).expect("workflow json parses");
        spec.validate().expect("workflow validates")
    }

    #[test]
    fn compiled_registry_registers_arbitrary_workflow_role_ids() {
        let compiled = workflow().compile();
        let registry: AgentRegistry<dyn Forge> = real_registry_from_compiled(provider(), &compiled)
            .expect("workflow has no required external tools");

        assert!(registry.contains_role(&RoleId::new("banana")));
        assert!(registry.contains_role(&RoleId::new("kumquat")));
    }

    #[test]
    fn compiled_registry_does_not_register_absent_reference_roles() {
        let compiled = workflow().compile();
        let registry: AgentRegistry<dyn Forge> = real_registry_from_compiled(provider(), &compiled)
            .expect("workflow has no required external tools");

        assert!(!registry.contains_role(&RoleId::new("engineer")));
        assert!(!registry.contains_role(&RoleId::new("architect")));
    }

    #[test]
    fn workflow_builder_compiles_once_and_registers_declared_roles() {
        let workflow = workflow();
        let registry: AgentRegistry<dyn Forge> = real_registry_from_workflow(provider(), &workflow)
            .expect("workflow has no required external tools");

        assert!(registry.contains_role(&RoleId::new("banana")));
        assert!(registry.contains_role(&RoleId::new("kumquat")));
        assert!(!registry.contains_role(&RoleId::new("reviewer")));
    }

    #[test]
    fn unbound_required_external_tool_fails_compiled_registry_preflight() {
        let compiled = required_tool_workflow().compile();
        let error = match real_registry_from_compiled::<dyn Forge>(provider(), &compiled) {
            Ok(_) => panic!("required tool needs a runner binding"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            ExternalToolBindingError::MissingRequired {
                role: RoleId::new("banana"),
                tool: "coding_workspace".into(),
            }
        );
    }
}
