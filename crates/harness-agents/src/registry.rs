//! Building [`AgentRegistry`] values for real, LLM-backed workflow roles.
//!
//! The production path is manifest-driven: given a compiled workflow, register
//! one generic [`LlmRoleAgent`](crate::LlmRoleAgent) for each compiled role. Role
//! ids and prompts therefore come from user workflow configuration rather than a
//! hard-coded reference-delivery list.
//!
//! The legacy `real_registry` / `real_registry_with` builders remain for
//! compatibility with existing reference-delivery real-agent tests until the
//! cleanup phase. They are not used by production workers.

use std::sync::Arc;

use harness_forge::Forge;
use harness_runner::{
    Agent, AgentRegistry, BoundExternalTool, ExternalToolBindingError, RunnerConfig,
};
use harness_workflow::{CompiledWorkflow, RoleId, RoleManifest, ValidatedWorkflow};

use crate::architect::LlmArchitect;
use crate::engineer::{EngineerPrep, LlmEngineer, NoPrep};
use crate::human::LlmHuman;
use crate::owner::LlmOwner;
use crate::provider::ProviderConfig;
use crate::reviewer::LlmReviewer;
use crate::role::LlmRoleAgent;

/// Which behavior variants and backend hooks the legacy reference-delivery
/// registry wires in.
///
/// Defaults reproduce the happy-path topology (non-closing architect, approving
/// reviewer, no engineer prep — i.e. the in-memory/filesystem backends). The
/// Forgejo test worker overrides `engineer_prep` and the scenarios that need
/// them set `architect_closing` / `reviewer_request_changes_then_approve`.
pub struct RealRegistryConfig<F: Forge + ?Sized> {
    /// When `true`, the architect also closes a merged PR's parent issues
    /// (`dependency_chain` scenario); mirrors `ClosingArchitect`.
    pub architect_closing: bool,
    /// When `true`, the reviewer requests changes on the first pass and approves
    /// on a later one; mirrors `RequestChangesThenApproveReviewer`.
    pub reviewer_request_changes_then_approve: bool,
    /// Backend side effects the engineer runs before opening a PR / addressing a
    /// CI failure (real git head, CI sentinel commit). [`NoPrep`] on
    /// memory/filesystem.
    pub engineer_prep: Arc<dyn EngineerPrep<F>>,
}

impl<F: Forge + ?Sized> Default for RealRegistryConfig<F> {
    fn default() -> Self {
        Self {
            architect_closing: false,
            reviewer_request_changes_then_approve: false,
            engineer_prep: Arc::new(NoPrep),
        }
    }
}

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
    config.validate_external_tool_bindings(compiled)?;
    register_compiled_roles(provider, compiled, |role| {
        config.bound_external_tools_for(role)
    })
}

fn register_compiled_roles<F>(
    provider: ProviderConfig,
    compiled: &CompiledWorkflow,
    bound_tools_for: impl Fn(&RoleManifest) -> Result<Vec<BoundExternalTool>, ExternalToolBindingError>,
) -> Result<AgentRegistry<F>, ExternalToolBindingError>
where
    F: Forge + ?Sized + 'static,
{
    let mut registry = AgentRegistry::new();
    for role in compiled.roles() {
        let bound_tools = bound_tools_for(role)?;
        registry.insert(
            role.id.clone(),
            Arc::new(LlmRoleAgent::with_bound_external_tools(
                role.clone(),
                provider.clone(),
                bound_tools,
            )) as Arc<dyn Agent<F>>,
        );
    }
    Ok(registry)
}

fn no_bound_external_tools(
    role: &RoleManifest,
) -> Result<Vec<BoundExternalTool>, ExternalToolBindingError> {
    if let Some(tool) = role.external_tools.iter().find(|tool| tool.required) {
        return Err(ExternalToolBindingError::MissingRequired {
            role: role.id.clone(),
            tool: tool.id.clone(),
        });
    }
    Ok(Vec::new())
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

/// Builds a legacy registry of real agents for every reference-delivery role,
/// with the default (happy-path) behavior variants and no engineer prep.
///
/// `F` is the Forge type the agents act over (`dyn Forge` in the worker). This
/// compatibility path is retained for reference-delivery tests until Phase 7;
/// production workers use [`real_registry_from_compiled`].
pub fn real_registry<F>(provider: ProviderConfig) -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
{
    real_registry_with(provider, RealRegistryConfig::default())
}

/// Builds the legacy reference-delivery real-agent registry with explicit
/// behavior variants and engineer prep.
pub fn real_registry_with<F>(
    provider: ProviderConfig,
    config: RealRegistryConfig<F>,
) -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
{
    let architect = if config.architect_closing {
        LlmArchitect::closing(provider.clone())
    } else {
        LlmArchitect::new(provider.clone())
    };
    let reviewer = if config.reviewer_request_changes_then_approve {
        LlmReviewer::request_changes_then_approve(provider.clone())
    } else {
        LlmReviewer::new(provider.clone())
    };
    let engineer = LlmEngineer::with_prep(provider.clone(), config.engineer_prep);

    let mut registry = AgentRegistry::new();
    registry.insert(
        RoleId::new("architect"),
        Arc::new(architect) as Arc<dyn Agent<F>>,
    );
    registry.insert(
        RoleId::new("engineer"),
        Arc::new(engineer) as Arc<dyn Agent<F>>,
    );
    registry.insert(
        RoleId::new("reviewer"),
        Arc::new(reviewer) as Arc<dyn Agent<F>>,
    );
    registry.insert(
        RoleId::new("owner"),
        Arc::new(LlmOwner::new(provider.clone())) as Arc<dyn Agent<F>>,
    );
    registry.insert(
        RoleId::new("human"),
        Arc::new(LlmHuman::new(provider)) as Arc<dyn Agent<F>>,
    );
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    use harness_workflow::RawWorkflowSpec;

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
