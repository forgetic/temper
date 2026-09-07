//! Explicit test-owned admission for plain-stdio fake providers. These fixtures
//! have no shared daemon; installed and mapped live tests cover shared lifetime.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use temper_agent::{AgentActivityConfig, CodingAgentError, ProviderConfig};
use temper_protocol_agent::{AgentToolConfig, WorkspaceContext, WorkspaceResult};

pub fn activity() -> AgentActivityConfig {
    AgentActivityConfig {
        lifecycle_reporter: Some(Arc::new(|_, _| {})),
        ..Default::default()
    }
}

pub async fn run_coding_agent_native_with_tool_config(
    handle: skein::runtime::RuntimeHandle,
    provider: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    tool_config: Option<&AgentToolConfig>,
) -> Result<WorkspaceResult, CodingAgentError> {
    temper_agent::run_coding_agent_native_with_totals_tool_config_and_hosts(
        handle,
        provider,
        context,
        cwd,
        max_iterations,
        config_dir,
        false,
        tool_config,
        Some(temper_agent::default_submit_for_pr_host()),
        None,
        activity(),
        Default::default(),
    )
    .await
    .map(|(result, _)| result)
}
