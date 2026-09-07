use super::*;

impl OutOfProcessRunner {
    pub(super) async fn prepare_provider(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> Result<
        (
            Option<AgentToolConfig>,
            Option<temper_codebase_memory_runtime::ProviderBootstrap>,
        ),
        AgentRunError,
    > {
        if self.runtime_limits.is_none()
            || !self
                .tool_config
                .as_ref()
                .is_some_and(|config| config.enabled_for_role(&context.work_item.role))
        {
            return Ok((self.tool_config.clone(), None));
        }
        // A separate parent factory, never the attempt's emergency registry.
        let factory = (self.containment_factory)("shared-provider", "account")
            .map_err(|_| AgentRunError::transient("prepare shared graph containment"))?;
        let helper = std::env::current_exe()
            .map_err(|_| AgentRunError::transient("resolve shared graph helper"))?;
        let manager = self.provider_owners.clone();
        let config = self.tool_config.clone();
        let environment = self.env.clone();
        let role = context.work_item.role.clone();
        let cwd = cwd.to_path_buf();
        crate::managed_effect::JoinedBlocking::spawn("shared-graph-admission", move || {
            temper_codebase_memory_runtime::prepare_invocation(
                &manager,
                config,
                &role,
                &cwd,
                &environment,
                &factory,
                &helper,
            )
        })
        .await
        .map_err(|_| AgentRunError::transient("shared graph admission owner failed"))?
        .map_err(|error| AgentRunError::transient(error.to_string()))
    }
}
