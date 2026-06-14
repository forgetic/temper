//! Production worker external-tool runtime wiring.

use std::sync::Arc;

use temper_runner::{
    CODING_WORKSPACE_TOOL_ID, ExternalToolExecutors, RunnerConfig, WorkspaceCheckout,
};
use temper_workflow::ExternalToolId;

use temper_coding_workspace::coding_workspace::LocalGitCodingWorkspace;

/// Conventional workspace external-tool ids and the checkout capability the
/// operator grants each. The engine never sees this mapping: it lives in the
/// worker wiring so the runtime stays role- and workflow-agnostic. An id not
/// listed here still binds (a workflow may name its own), defaulting to a
/// writable checkout so an unrecognized workspace keeps today's head behavior.
const TRIAGE_WORKSPACE_TOOL_ID: &str = "triage_workspace";
const REVIEW_WORKSPACE_TOOL_ID: &str = "review_workspace";

/// Whether the local-git workspace provider can back a declared external-tool
/// id. The provider prepares a git checkout, so it backs the workspace tools
/// (`coding_workspace` and any `*_workspace` id an operator names) and leaves
/// other external-tool kinds (read-only metadata tools, etc.) unbound for a
/// different provider. This keeps the binding from grabbing tools the local-git
/// provider has no business serving.
fn is_workspace_tool_id(tool_id: &str) -> bool {
    tool_id == CODING_WORKSPACE_TOOL_ID || tool_id.ends_with("_workspace")
}

/// Returns the checkout capability for a declared workspace tool id.
///
/// Different tool ids need different checkouts: the engineer's `coding_workspace`
/// writes a head, the architect's `triage_workspace` only reads, and the
/// reviewer's `review_workspace` reads the pull request's head and base to
/// compute the real diff. Keyed off the tool id, not the role, so the provider
/// and engine stay role-agnostic.
fn checkout_for_tool_id(tool_id: &str) -> WorkspaceCheckout {
    match tool_id {
        REVIEW_WORKSPACE_TOOL_ID => WorkspaceCheckout::PullRequestReadOnly,
        TRIAGE_WORKSPACE_TOOL_ID => WorkspaceCheckout::ReadOnly,
        // `coding_workspace` and any operator-named id default to writable.
        _ => WorkspaceCheckout::Writable,
    }
}

pub(crate) fn configure_external_tool_executors(
    compiled: &temper_workflow::CompiledWorkflow,
    config: &mut RunnerConfig,
) -> Result<ExternalToolExecutors, String> {
    configure_external_tool_executors_with_env(compiled, config, |key| std::env::var(key).ok())
}

/// Env-injected core of [`configure_external_tool_executors`] so tests drive the
/// workspace binding without mutating process environment.
fn configure_external_tool_executors_with_env<E>(
    compiled: &temper_workflow::CompiledWorkflow,
    config: &mut RunnerConfig,
    env: E,
) -> Result<ExternalToolExecutors, String>
where
    E: Fn(&str) -> Option<String>,
{
    let mut executors = ExternalToolExecutors::new();
    let Some(workspace) = LocalGitCodingWorkspace::from_env(env)? else {
        return Ok(executors);
    };
    let workspace = Arc::new(workspace) as Arc<dyn temper_runner::CodingWorkspace>;

    // Bind the workspace provider for *every* external-tool id declared by a
    // compiled role (e.g. `triage_workspace`, `coding_workspace`,
    // `review_workspace`), not just `coding_workspace`. Dedupe per `(role, id)`
    // so a role declaring the same id twice binds once.
    let mut bound = 0usize;
    for role in compiled.roles() {
        let mut seen: Vec<&str> = Vec::new();
        for declared in &role.external_tools {
            let id = declared.id.as_str();
            if !is_workspace_tool_id(id) || seen.contains(&id) {
                continue;
            }
            seen.push(id);

            let tool = ExternalToolId::new(id);
            config.set_external_tool_binding(
                role.id.clone(),
                tool.clone(),
                "local-git-coding-workspace",
            );
            executors.add_workspace_checkout(
                role.id.clone(),
                tool,
                Arc::clone(&workspace),
                checkout_for_tool_id(id),
            );
            bound += 1;
        }
    }
    if bound == 0 {
        eprintln!(
            concat!(
                "temper-worker: coding workspace env is configured, but no compiled ",
                "role declares a workspace external tool (e.g. {})"
            ),
            CODING_WORKSPACE_TOOL_ID
        );
    }
    Ok(executors)
}

#[cfg(test)]
#[path = "worker_external_tools_tests.rs"]
mod tests;
