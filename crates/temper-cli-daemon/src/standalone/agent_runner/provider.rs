use std::path::Path;

use temper_codebase_memory_runtime::{ProviderBootstrap, ProviderOwnerManager};
use temper_process_containment::ContainmentFactory;
use temper_protocol_agent::AgentToolConfig;
use temper_worker::AgentRunError;

pub(super) async fn prepare(
    manager: ProviderOwnerManager,
    config: Option<AgentToolConfig>,
    role: &str,
    cwd: &Path,
    factory: ContainmentFactory,
) -> Result<(Option<AgentToolConfig>, Option<ProviderBootstrap>), AgentRunError> {
    if !config
        .as_ref()
        .is_some_and(|config| config.enabled_for_role(role))
    {
        return Ok((config, None));
    }
    let helper = std::env::current_exe()
        .map_err(|_| AgentRunError::transient("resolve shared graph helper"))?;
    let role = role.to_string();
    let cwd = cwd.to_path_buf();
    skein::runtime::spawn_blocking(move || {
        temper_codebase_memory_runtime::prepare_invocation(
            &manager,
            config,
            &role,
            &cwd,
            &[],
            &factory,
            &helper,
        )
    })
    .await
    .map_err(|error| AgentRunError::transient(error.to_string()))
}
