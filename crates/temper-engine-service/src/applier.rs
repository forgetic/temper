// SPDX-License-Identifier: MPL-2.0

//! Result applier construction for the engine service.

use std::collections::BTreeMap;
use std::sync::Arc;

use temper_config::{ExposeSecret, Secret};
use temper_engine::{
    DaemonRunConfig, ForgeApplier, LeaseApplier, ResultApplier, RoleRoutingApplier,
};
use temper_forge::Forge;
use temper_forge::config::ForgejoConfig;
use temper_workflow::LeasePolicy;

/// Builds the result applier chain, routing each role's writes through that
/// role's forge identity when a per-role token is available (otherwise the
/// default identity).
pub fn result_applier(
    default_forge: Arc<dyn Forge>,
    forge_base_url: String,
    workflow: Arc<temper_workflow::ValidatedWorkflow>,
    config: &DaemonRunConfig,
    role_tokens: &BTreeMap<String, Secret>,
    lease_ttl: chrono::Duration,
) -> Arc<dyn ResultApplier> {
    let default_chain = applier_chain(
        default_forge,
        workflow.clone(),
        config.daemon_id.clone(),
        lease_ttl,
    );
    if role_tokens.is_empty() {
        return default_chain;
    }

    let mut routing = RoleRoutingApplier::new(default_chain);
    let mut routed = Vec::new();
    let mut fallback = Vec::new();

    for role in &config.roles {
        let role = role.as_str().to_string();
        if let Some(token) = role_tokens.get(&role) {
            // I/O boundary: the per-role token is handed to its Forgejo client.
            let role_forge = temper_forge::factory::new_forgejo(ForgejoConfig::new(
                forge_base_url.clone(),
                token.expose_secret(),
            ));
            let role_chain = applier_chain(
                role_forge,
                workflow.clone(),
                config.daemon_id.clone(),
                lease_ttl,
            );
            routing = routing.with_route(role.clone(), role_chain);
            routed.push(role);
        } else {
            fallback.push(role);
        }
    }

    eprintln!(
        "temper-engine: role identities routed={} fallback={}",
        role_list(&routed),
        role_list(&fallback)
    );

    Arc::new(routing)
}

fn applier_chain(
    forge: Arc<dyn Forge>,
    workflow: Arc<temper_workflow::ValidatedWorkflow>,
    daemon_id: String,
    lease_ttl: chrono::Duration,
) -> Arc<dyn ResultApplier> {
    Arc::new(LeaseApplier::new(
        forge.clone(),
        LeasePolicy::new(lease_ttl),
        daemon_id,
        Arc::new(ForgeApplier::new(forge, workflow)),
        temper_engine::system_clock(),
    ))
}

fn role_list(roles: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let roles = roles
        .into_iter()
        .map(|role| role.as_ref().to_string())
        .collect::<Vec<_>>();
    if roles.is_empty() {
        "(none)".to_string()
    } else {
        roles.join(",")
    }
}
