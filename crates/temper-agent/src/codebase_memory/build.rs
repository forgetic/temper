//! Explicit lifecycle authority at toolset admission.
use super::*;

/// Legacy unmanaged entry point. Configured graph tools fail closed because
/// this entry point has no shared provider launch authority. Use
/// [`build_managed_codebase_memory_toolset`] with caller-owned admission.
///
/// Error behavior is mode-dependent:
///
/// - absent config or a role mismatch returns an empty, disabled toolset;
/// - configured `mode = auto` returns an empty `AutoUnavailable` toolset;
/// - configured `mode = required` returns [`CodebaseMemoryToolsetError`].
///
/// The agent-callable tools are workspace-scoped: `project`/`repo` inputs are
/// resolved only against aliases derived from [`WorkspaceContext::repos`], and
/// internal `index_repository` calls are made only for those prepared repo roots.
pub async fn build_codebase_memory_toolset(
    config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
) -> std::result::Result<CodebaseMemoryToolset, CodebaseMemoryToolsetError> {
    build_codebase_memory_toolset_with_timeout(config, role, context, cwd, Duration::MAX).await
}

/// Unmanaged entry point with a tool deadline; shares the fail-closed behavior
/// of [`build_codebase_memory_toolset`].
pub async fn build_codebase_memory_toolset_with_timeout(
    config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
    generic_tool_timeout: Duration,
) -> std::result::Result<CodebaseMemoryToolset, CodebaseMemoryToolsetError> {
    let containment = default_containment_context();
    build_codebase_memory_toolset_with_timeout_and_containment(
        config,
        role,
        context,
        cwd,
        generic_tool_timeout,
        &containment,
        None,
    )
    .await
}

fn default_containment_context() -> AgentContainmentContext {
    #[cfg(test)]
    {
        crate::containment_tests::containment_context()
    }
    #[cfg(not(test))]
    AgentContainmentContext::production(None)
}

/// Builds within explicit parent-owned admission. The caller creates this
/// bootstrap outside job containment using operator-resolved configuration;
/// it must not release the same admission from another concurrent invocation.
/// This function retains bootstrap demand until serving admission or joined
/// startup cleanup, and fences a delayed or missing serving connection.
#[allow(clippy::too_many_arguments)]
pub async fn build_managed_codebase_memory_toolset(
    config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
    generic_tool_timeout: Duration,
    containment: &AgentContainmentContext,
    bootstrap: temper_codebase_memory_runtime::ProviderBootstrap,
) -> std::result::Result<CodebaseMemoryToolset, CodebaseMemoryToolsetError> {
    if !bootstrap.admission_pending() {
        return unavailable_admission(config);
    }
    // MCP Drop requests cleanup on a separate owner. Retain admission in each
    // spawned containment coordinator, including when this future is dropped.
    // That coordinator outlives recursive cleanup and the MCP reader join.
    let containment = containment.clone().with_observer(
        temper_codebase_memory_runtime::retain_admission_until_cleanup(bootstrap.clone()),
    );
    let admitted = || bootstrap.serving_admitted();
    let build = Box::pin(build_codebase_memory_toolset_with_timeout_and_containment(
        config,
        role,
        context,
        cwd,
        generic_tool_timeout,
        &containment,
        Some(&admitted),
    ));
    let deadline = Box::pin(temper_agent_io::sleep_for(bootstrap.admission_remaining()));
    match futures::future::select(build, deadline).await {
        futures::future::Either::Left((result, _)) => result,
        futures::future::Either::Right((_, build)) => {
            if bootstrap.expire_admission() {
                // Stop future launches. Already-spawned clients retain their
                // own admission until the independent cleanup owner finishes.
                drop(build);
                unavailable_admission(config)
            } else {
                build.await
            }
        }
    }
}

fn unavailable_admission(
    config: Option<&AgentToolConfig>,
) -> std::result::Result<CodebaseMemoryToolset, CodebaseMemoryToolsetError> {
    let reason = "codebase-memory requires active parent-owned provider admission";
    if config
        .and_then(|config| config.codebase_memory.as_ref())
        .is_some_and(|config| config.mode == CodebaseMemoryMode::Required)
    {
        Err(CodebaseMemoryToolsetError::required_setup(reason))
    } else {
        Ok(CodebaseMemoryToolset::disabled(
            CodebaseMemoryToolsetStatus::AutoUnavailable {
                reason: reason.into(),
            },
        ))
    }
}

pub(crate) async fn build_codebase_memory_toolset_with_timeout_and_containment(
    config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
    generic_tool_timeout: Duration,
    containment: &AgentContainmentContext,
    serving_admitted: Option<&(dyn Fn() + Send + Sync)>,
) -> std::result::Result<CodebaseMemoryToolset, CodebaseMemoryToolsetError> {
    let Some(codebase_memory) = config.and_then(|config| config.codebase_memory.as_ref()) else {
        return Ok(CodebaseMemoryToolset::disabled(
            CodebaseMemoryToolsetStatus::NotConfigured,
        ));
    };
    if !codebase_memory.applies_to_role(role) {
        return Ok(CodebaseMemoryToolset::disabled(
            CodebaseMemoryToolsetStatus::NotEnabledForRole {
                role: role.to_string(),
            },
        ));
    }

    if serving_admitted.is_none() {
        return unavailable_admission(config);
    }

    let scope = match WorkspaceScope::from_context(context, cwd) {
        Ok(scope) => scope,
        Err(error) if codebase_memory.mode == CodebaseMemoryMode::Auto => {
            return Ok(CodebaseMemoryToolset::disabled(
                CodebaseMemoryToolsetStatus::AutoUnavailable { reason: error },
            ));
        }
        Err(error) => {
            return Err(CodebaseMemoryToolsetError::required_setup(format!(
                "required codebase-memory workspace scope failed: {error}"
            )));
        }
    };

    match start_toolset(
        codebase_memory,
        role,
        scope,
        generic_tool_timeout,
        containment,
        serving_admitted,
    )
    .await
    {
        Ok(toolset) => Ok(toolset),
        Err(error) if codebase_memory.mode == CodebaseMemoryMode::Auto => Ok(
            CodebaseMemoryToolset::disabled(CodebaseMemoryToolsetStatus::AutoUnavailable {
                reason: error.to_string(),
            }),
        ),
        Err(error) => Err(CodebaseMemoryToolsetError::required_startup(error)),
    }
}
