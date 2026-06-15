// SPDX-License-Identifier: MPL-2.0

//! Adapters from a [`Resolved`] config into the engine tier's runtime types.
//!
//! These are pure (no I/O) and `pub` so the unified binary's standalone mode can
//! reuse the exact same translation the slim `temper-engine` binary uses.

use temper_config::Resolved;
use temper_engine::DaemonRunConfig;
use temper_forge_model::RepositoryPath;
use temper_forge_forgejo::ForgejoConfig;
use temper_workflow::RoleId;

/// Builds the Forgejo backend config from the resolved forge settings.
///
/// Requires a forge URL and an admin token; applies the optional CI-reader
/// web-UI credentials (ADR 0019).
pub fn forgejo_config(resolved: &Resolved) -> Result<ForgejoConfig, String> {
    let url = resolved
        .forge
        .require_url()
        .map_err(|error| error.to_string())?;
    let token = resolved
        .forge
        .require_admin_token()
        .map_err(|error| error.to_string())?;
    let mut config = ForgejoConfig::new(url, token);
    if let Some(web) = &resolved.forge.web_ui {
        config = config.with_web_ui_credentials(web.username.clone(), web.password.clone());
    }
    Ok(config)
}

/// Builds the daemon runtime config from the resolved engine settings.
///
/// Requires at least one repository and one role.
pub fn daemon_run_config(resolved: &Resolved) -> Result<DaemonRunConfig, String> {
    let engine = &resolved.engine;
    let repos = engine
        .require_repos()
        .map_err(|error| error.to_string())?
        .iter()
        .map(|repo| RepositoryPath::new(repo.owner.clone(), repo.name.clone()))
        .collect();
    let roles = engine
        .require_roles()
        .map_err(|error| error.to_string())?
        .iter()
        .map(RoleId::new)
        .collect();
    Ok(DaemonRunConfig {
        bind: engine.bind,
        repos,
        roles,
        workflow_file: engine.workflow_file.clone(),
        poll_cadence: engine.poll_cadence,
        mechanical_cadence: engine.mechanical_cadence,
        lease_ttl: engine.lease_ttl,
        webhook_secret_file: engine.webhook_secret_file.clone(),
        daemon_id: engine.daemon_id.clone(),
    })
}
