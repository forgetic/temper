// SPDX-License-Identifier: MPL-2.0

//! Adapters from a [`Resolved`] config into the engine tier's runtime types.
//!
//! These are pure (no I/O) and `pub` so the unified binary's standalone mode can
//! reuse the exact same translation the slim `temper-engine` binary uses.

use temper_config::{ExposeSecret, Resolved, WorkerPoolSettings};
use temper_engine::{
    DaemonRunConfig, EngineAgentTraceConfig, EngineConfig, WorkerAuth, WorkerPoolAuthConfig,
    WorkerPoolPolicy,
};
use temper_forge::RepositoryPath;
use temper_forge::config::ForgejoConfig;
use temper_workflow::{CompiledWorkflow, RoleId};

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
    // I/O boundary: the token is handed to the Forgejo HTTP client.
    let mut config = ForgejoConfig::new(url, token.expose_secret());
    if let Some(web) = &resolved.forge.web_ui {
        config = config.with_web_ui_credentials(
            web.username.clone(),
            web.password.expose_secret().to_string(),
        );
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
        ci_poll_cadence: engine.ci_poll_cadence,
        mechanical_cadence: engine.mechanical_cadence,
        lease_ttl: engine.lease_ttl,
        webhook_secret_file: engine.webhook_secret_file.clone(),
        daemon_id: engine.daemon_id.clone(),
        worker_pools: worker_pool_policies(&resolved.worker.pools),
    })
}

/// Projects the compiled workflow's finite per-role concurrency limits into
/// the daemon scheduler's role-limit map. Roles with `None` are deliberately
/// absent: the daemon interprets an absent role as unlimited and continues to
/// schedule it according to available worker capacity.
pub fn workflow_role_limits(
    compiled: &CompiledWorkflow,
) -> std::collections::BTreeMap<String, u32> {
    compiled
        .roles()
        .iter()
        .filter_map(|role| {
            role.concurrency
                .map(|limit| (role.id.as_str().to_string(), limit))
        })
        .collect()
}

fn worker_pool_policies(pools: &[WorkerPoolSettings]) -> Vec<WorkerPoolPolicy> {
    pools
        .iter()
        .map(|pool| {
            WorkerPoolPolicy::new(
                pool.name.clone(),
                pool.roles.clone(),
                pool.repos.iter().map(|repo| repo.display()).collect(),
                pool.max_concurrent_jobs,
            )
        })
        .collect()
}

/// Builds the engine's per-subsystem config object from a resolved deployment.
///
/// Bundles the daemon run config, the default Forgejo client config, and the
/// per-role REST tokens the result applier routes writes through, so the engine
/// runtime stands the daemon up from one struct. The tokens stay wrapped in
/// [`SecretString`](secrecy::SecretString) here — they are only exposed at the
/// true I/O boundary where each per-role Forgejo client is built.
pub fn engine_config(resolved: &Resolved) -> Result<EngineConfig, String> {
    let forge = forgejo_config(resolved)?;
    let daemon = daemon_run_config(resolved)?;
    let role_tokens = resolved.forge.role_tokens.clone();
    let traces = &resolved.observability.agent_traces;
    let agent_traces = EngineAgentTraceConfig {
        policy: traces.policy_for_storage(traces.engine_journal_root.as_deref()),
        journal_root: traces.engine_journal_root.clone(),
        read_token: traces.read_token_value.clone(),
    };
    Ok(EngineConfig::new(daemon, forge, role_tokens).with_agent_traces(agent_traces))
}

/// Builds the daemon's worker-pool authentication policy from resolved pools.
/// Pools without `worker_token` remain known but unauthenticated; pools with a
/// token require the resolved non-empty secret payload.
pub fn worker_pool_auth_config(resolved: &Resolved) -> Result<WorkerPoolAuthConfig, String> {
    let mut config = WorkerPoolAuthConfig::new();
    for pool in &resolved.worker.pools {
        let token = match pool.worker_token.as_ref() {
            Some(reference) => {
                let value = resolved.worker.worker_pool_tokens.get(&pool.name).ok_or_else(|| {
                    format!(
                        "worker pool `{}` worker_token references secret `{}` but it has no non-empty text value",
                        pool.name, reference.name
                    )
                })?;
                Some(WorkerAuth::bearer(value.expose_secret().to_string()))
            }
            None => None,
        };
        config.insert_pool(pool.name.clone(), token);
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;

    use temper_config::Secret;
    use temper_engine::DaemonRunConfig;
    use temper_forge::RepositoryPath;
    use temper_workflow::RoleId;

    use super::*;
    use crate::result_applier;

    fn compiled_workflow_with_mixed_limits() -> CompiledWorkflow {
        let mut spec = temper_workflow::parse_workflow_spec(
            "workflow.json",
            temper_reference_delivery::reference_delivery_workflow_json(),
        )
        .expect("reference workflow parses");
        spec.roles[0].concurrency = Some(2);
        spec.roles[1].concurrency = None;
        spec.roles[2].concurrency = Some(5);
        spec.validate().expect("workflow validates").compile()
    }

    #[test]
    fn workflow_role_limits_projects_only_finite_compiled_limits() {
        let compiled = compiled_workflow_with_mixed_limits();

        let limits = workflow_role_limits(&compiled);

        assert_eq!(limits.get("architect"), Some(&2));
        assert_eq!(limits.get("reviewer"), Some(&5));
        assert_eq!(limits.get("engineer"), None);
        assert_eq!(limits.len(), compiled.roles().len() - 1);
    }

    fn in_memory_engine_config() -> EngineConfig {
        let daemon = DaemonRunConfig {
            bind: "127.0.0.1:8080".parse().expect("valid bind"),
            repos: vec![RepositoryPath::new("acme", "widgets")],
            roles: vec![RoleId::new("coder")],
            workflow_file: None,
            poll_cadence: Duration::from_secs(30),
            ci_poll_cadence: Some(Duration::from_secs(60)),
            mechanical_cadence: Some(Duration::from_secs(120)),
            lease_ttl: Duration::from_secs(300),
            webhook_secret_file: None,
            daemon_id: "engine-test".to_string(),
            worker_pools: Vec::new(),
        };
        let forge = ForgejoConfig::new("https://forge.example", "admin-token");
        let mut role_tokens = BTreeMap::new();
        role_tokens.insert("coder".to_string(), Secret::from("coder-token"));
        EngineConfig::new(daemon, forge, role_tokens)
    }

    #[test]
    fn daemon_adapter_preserves_the_resolved_ci_poll_cadence() {
        let config = temper_config::Config {
            engine: temper_config::EngineConfig {
                repos: Some(vec!["acme/widgets".to_string()]),
                roles: Some(vec!["engineer".to_string()]),
                ci_poll_cadence_secs: Some(19),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = temper_config::resolve(
            &config,
            &temper_config::Credentials::default(),
            &temper_config::NoEnv,
        )
        .expect("config resolves");

        let daemon = daemon_run_config(&resolved).expect("daemon config adapts");

        assert_eq!(daemon.ci_poll_cadence, Some(Duration::from_secs(19)));
        assert_eq!(daemon.poll_cadence, Duration::from_secs(300));
        assert_eq!(daemon.mechanical_cadence, Some(Duration::from_secs(120)));
    }

    /// The result-applier factory accepts an in-memory [`EngineConfig`]: a
    /// subsystem test stands the engine's applier seam up with no files/env/args,
    /// driving a memory forge.
    #[test]
    fn result_applier_accepts_in_memory_engine_config() {
        let config = in_memory_engine_config();
        let forge = temper_forge::factory::new_memory();
        let workflow = Arc::new(
            temper_reference_delivery::resolve_workflow(None::<&std::path::Path>)
                .expect("default workflow resolves"),
        );
        let lease_ttl = chrono::Duration::from_std(config.daemon.lease_ttl).expect("valid ttl");

        // The factory accepts the bundled daemon config + per-role tokens.
        let _applier = result_applier(
            forge,
            config.forge.clone(),
            workflow,
            &config.daemon,
            &config.role_tokens,
            lease_ttl,
        );
    }
}
