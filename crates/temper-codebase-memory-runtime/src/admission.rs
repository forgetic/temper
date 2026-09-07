use std::path::Path;
use std::time::Duration;

use temper_process_containment::ContainmentFactory;
use temper_protocol_agent::{AgentToolConfig, CodebaseMemoryMode};

use crate::{LaunchConfig, ProviderBootstrap, ProviderBootstrapFailure, ProviderOwnerManager};

/// Prepare one operator-configured invocation. A failed optional bootstrap
/// removes graph tools only from this invocation, preventing an unsafe cold
/// retry inside its job. Required mode preserves the bounded startup failure.
pub fn prepare_invocation(
    manager: &ProviderOwnerManager,
    mut config: Option<AgentToolConfig>,
    role: &str,
    workspace: &Path,
    environment: &[(String, String)],
    factory: &ContainmentFactory,
    helper: &Path,
) -> Result<(Option<AgentToolConfig>, Option<ProviderBootstrap>), ProviderBootstrapFailure> {
    let Some(provider) = config
        .as_ref()
        .and_then(|value| value.codebase_memory.as_ref())
        .filter(|provider| provider.applies_to_role(role))
    else {
        return Ok((config, None));
    };
    let required = provider.mode == CodebaseMemoryMode::Required;
    let launch = LaunchConfig {
        command: provider.command.clone(),
        args: provider.args.clone(),
        workspace: workspace.to_path_buf(),
        environment: environment.to_vec(),
        startup_timeout: Duration::from_secs(provider.startup_timeout_secs),
        admission_timeout: Duration::from_secs(
            provider
                .startup_timeout_secs
                .saturating_mul(4)
                .max(30)
                .min(86_400),
        ),
    };
    match ProviderBootstrap::start(manager, launch, factory, helper) {
        Ok(bootstrap) => Ok((config, Some(bootstrap))),
        Err(error) if required => Err(error),
        Err(error) => {
            tracing::warn!(target: "temper::codebase_memory", event = "codebase_memory.shared_owner", lifecycle.stage = "unavailable", category = %error,
                "optional graph bootstrap unavailable for this invocation");
            if let Some(config) = config.as_mut() {
                config.codebase_memory = None;
            }
            Ok((config, None))
        }
    }
}
