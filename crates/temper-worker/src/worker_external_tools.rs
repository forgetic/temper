//! Production worker external-tool runtime wiring.

use std::sync::Arc;

use temper_runner::{ExternalToolExecutors, RunnerConfig, CODING_WORKSPACE_TOOL_ID};
use temper_workflow::{CompiledWorkflow, ExternalToolId};

use temper_coding_workspace::coding_workspace::LocalGitCodingWorkspace;

pub(crate) fn configure_external_tool_executors(
    compiled: &CompiledWorkflow,
    config: &mut RunnerConfig,
) -> Result<ExternalToolExecutors, String> {
    let mut executors = ExternalToolExecutors::new();
    let Some(workspace) = LocalGitCodingWorkspace::from_env(|key| std::env::var(key).ok())? else {
        return Ok(executors);
    };
    let workspace = Arc::new(workspace) as Arc<dyn temper_runner::CodingWorkspace>;
    let mut bound = 0usize;
    for role in compiled.roles() {
        if role
            .external_tools
            .iter()
            .any(|tool| tool.id.as_str() == CODING_WORKSPACE_TOOL_ID)
        {
            let tool = ExternalToolId::new(CODING_WORKSPACE_TOOL_ID);
            config.set_external_tool_binding(
                role.id.clone(),
                tool.clone(),
                "local-git-coding-workspace",
            );
            executors.add_workspace(role.id.clone(), tool, Arc::clone(&workspace));
            bound += 1;
        }
    }
    if bound == 0 {
        eprintln!(
            concat!(
                "temper-worker: coding workspace env is configured, but no compiled ",
                "role declares {}"
            ),
            CODING_WORKSPACE_TOOL_ID
        );
    }
    Ok(executors)
}
