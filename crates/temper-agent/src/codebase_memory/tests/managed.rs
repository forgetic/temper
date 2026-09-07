//! Explicit test-owned admission for isolated fake providers. Real shared owner
//! lifecycle is exercised by the dedicated runtime and installed-provider tests.
use super::*;

pub(in crate::codebase_memory) async fn build_codebase_memory_toolset(
    config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
) -> std::result::Result<CodebaseMemoryToolset, CodebaseMemoryToolsetError> {
    build_codebase_memory_toolset_with_timeout(config, role, context, cwd, Duration::MAX).await
}

pub(in crate::codebase_memory) async fn build_codebase_memory_toolset_with_timeout(
    config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
    generic_tool_timeout: Duration,
) -> std::result::Result<CodebaseMemoryToolset, CodebaseMemoryToolsetError> {
    let containment = crate::containment_tests::containment_context();
    super::super::build_codebase_memory_toolset_with_timeout_and_containment(
        config,
        role,
        context,
        cwd,
        generic_tool_timeout,
        &containment,
        Some(&|| {}),
    )
    .await
}
