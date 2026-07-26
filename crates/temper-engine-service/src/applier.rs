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

/// Builds the Forge clients used to route role-authored mutations.
///
/// The returned map retains authenticated clients in process only. Durable
/// workflow state records role identifiers, never tokens, and startup recovery
/// resolves those identifiers through this same configured map.
pub fn configured_role_forges(
    forge_config: &ForgejoConfig,
    config: &DaemonRunConfig,
    role_tokens: &BTreeMap<String, Secret>,
) -> BTreeMap<String, Arc<dyn Forge>> {
    config
        .roles
        .iter()
        .filter_map(|role| {
            let role = role.as_str().to_string();
            let token = role_tokens.get(&role)?;
            // I/O boundary: the per-role token is handed to its Forgejo client.
            // Preserve every non-token setting from the deployment config.
            let forge: Arc<dyn Forge> =
                temper_forge::factory::new_forgejo(role_forgejo_config(forge_config, token));
            Some((role, forge))
        })
        .collect()
}

/// Builds the result applier chain, routing each role's writes through that
/// role's forge identity when a per-role client is available (otherwise the
/// default identity).
pub fn result_applier(
    default_forge: Arc<dyn Forge>,
    role_forges: &BTreeMap<String, Arc<dyn Forge>>,
    workflow: Arc<temper_workflow::ValidatedWorkflow>,
    config: &DaemonRunConfig,
    lease_ttl: chrono::Duration,
) -> Arc<dyn ResultApplier> {
    let default_chain = applier_chain(
        default_forge,
        workflow.clone(),
        config.daemon_id.clone(),
        lease_ttl,
    );
    if role_forges.is_empty() {
        return default_chain;
    }

    let mut routing = RoleRoutingApplier::new(default_chain);
    let mut routed = Vec::new();
    let mut fallback = Vec::new();

    for role in &config.roles {
        let role = role.as_str().to_string();
        if let Some(role_forge) = role_forges.get(&role) {
            let role_chain = applier_chain(
                role_forge.clone(),
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

    // Per-role applier routing is setup detail, not a §7 state change; keep it at
    // debug so `RUST_LOG=info` stays the §7 catalog + startup banner.
    let routed = role_list(&routed);
    let fallback = role_list(&fallback);
    let message = role_identities_debug_message(&routed, &fallback);
    tracing::debug!(
        target: "temper::engine",
        service = temper_log::Service::Engine.as_str(),
        %routed,
        %fallback,
        "{message}"
    );

    Arc::new(routing)
}

fn role_forgejo_config(base: &ForgejoConfig, token: &Secret) -> ForgejoConfig {
    let mut config = base.clone();
    config.token = token.expose_secret().to_string();
    config
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

fn role_identities_debug_message(routed: &str, fallback: &str) -> String {
    format!(
        "{}role identities routed={routed} fallback={fallback}",
        temper_log::Service::Engine.human_prefix()
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_forgejo_config_preserves_settings_and_replaces_only_token() {
        let base = ForgejoConfig::new("https://forge.example/", "admin-token")
            .with_default_repo("acme", "widgets")
            .with_page_limit(7)
            .with_cas_mode(temper_forge::config::ForgejoCasMode::Strict);

        let role = role_forgejo_config(&base, &Secret::from("role-token"));
        let mut expected = base.clone();
        expected.token = "role-token".to_string();

        assert_eq!(role, expected);
        assert_eq!(base.token, "admin-token");
    }

    #[test]
    fn role_identities_debug_message_uses_padded_engine_prefix() {
        let message = role_identities_debug_message("architect,engineer", "(none)");

        assert_eq!(
            message,
            "engine:  role identities routed=architect,engineer fallback=(none)"
        );
        assert_eq!(&message[.."engine:  ".len()], "engine:  ");
    }
}
