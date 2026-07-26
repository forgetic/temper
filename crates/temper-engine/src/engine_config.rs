// SPDX-License-Identifier: MPL-2.0

//! The engine's per-subsystem config object.
//!
//! [`EngineConfig`] bundles everything the engine-service runtime needs to stand
//! the daemon up: the parsed [`DaemonRunConfig`], the [`ForgejoConfig`] for the
//! default forge client, and the per-role REST tokens the result-application
//! seam routes writes through. Grouping these into one struct keeps the
//! `engine_config(&Resolved)` adapter and the runtime entry point from carrying
//! long, growing parameter lists (the per-subsystem config-object rule — see the
//! crate root docs).
//!
//! The struct is constructible **in memory** with no files/env/args, so unit and
//! integration tests can build an engine config directly.

use std::collections::BTreeMap;
use std::path::PathBuf;

use secrecy::SecretString;
use temper_forge::config::ForgejoConfig;
use temper_protocol_activity::{AgentActivityCapturePolicyV1, CaptureModeV1};

use crate::DaemonRunConfig;

/// Everything the engine service needs to build its daemon, in one struct.
///
/// Built by `temper_engine_service::engine_config(&Resolved)` from a resolved
/// deployment, or directly in tests. The role-token applier inputs live here
/// (`role_tokens`) so the runtime's daemon wiring takes a single config rather
/// than threading the tokens separately.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// The parsed daemon runtime config (bind, repos, roles, cadences, …).
    pub daemon: DaemonRunConfig,
    /// The default token-authenticated Forgejo API client config.
    pub forge: ForgejoConfig,
    /// Role id -> REST token, for the per-role routing applier. Absent roles fall
    /// back to the default (admin) identity. Tokens stay wrapped in
    /// [`SecretString`] until the true I/O boundary where the per-role Forgejo
    /// client is built.
    pub role_tokens: BTreeMap<String, SecretString>,
    /// Effective trace policy, durable journal root, and query credential.
    pub agent_traces: EngineAgentTraceConfig,
}

/// Engine-owned durable trace-journal configuration.
///
/// The query bearer remains a `SecretString`, so cloning/debugging the subsystem
/// config cannot expose it. A missing journal root always pairs with an effective
/// `off` policy; the service emits an operator-visible warning at startup.
#[derive(Clone, Debug)]
pub struct EngineAgentTraceConfig {
    pub policy: AgentActivityCapturePolicyV1,
    pub journal_root: Option<PathBuf>,
    pub read_token: Option<SecretString>,
}

impl Default for EngineAgentTraceConfig {
    fn default() -> Self {
        let policy = AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Off,
            ..Default::default()
        };
        Self {
            policy,
            journal_root: None,
            read_token: None,
        }
    }
}

impl EngineConfig {
    /// Bundles the daemon config, forge config, and per-role tokens.
    pub fn new(
        daemon: DaemonRunConfig,
        forge: ForgejoConfig,
        role_tokens: BTreeMap<String, SecretString>,
    ) -> Self {
        Self {
            daemon,
            forge,
            role_tokens,
            agent_traces: EngineAgentTraceConfig::default(),
        }
    }

    /// Installs the resolved engine trace journal configuration.
    #[must_use]
    pub fn with_agent_traces(mut self, agent_traces: EngineAgentTraceConfig) -> Self {
        self.agent_traces = agent_traces;
        self
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use secrecy::SecretString;
    use temper_forge::RepositoryPath;
    use temper_workflow::RoleId;

    use super::*;

    /// The engine config is constructible entirely in memory — no files, env, or
    /// args — so a subsystem test can build the daemon's inputs directly.
    #[test]
    fn engine_config_builds_in_memory() {
        let daemon = DaemonRunConfig {
            bind: "127.0.0.1:8080".parse().expect("valid bind"),
            repos: vec![RepositoryPath::new("acme", "widgets")],
            roles: vec![RoleId::new("coder")],
            workflow_file: None,
            poll_cadence: Duration::from_secs(30),
            ci_poll_cadence: Some(Duration::from_secs(60)),
            ci_missing_grace: Duration::from_secs(300),
            mechanical_cadence: Some(Duration::from_secs(120)),
            lease_ttl: Duration::from_secs(300),
            webhook_secret_file: None,
            daemon_id: "engine-test".to_string(),
            worker_pools: Vec::new(),
        };
        let forge = ForgejoConfig::new("https://forge.example", "admin-token");
        let mut role_tokens = BTreeMap::new();
        role_tokens.insert("coder".to_string(), SecretString::from("coder-token"));

        let config = EngineConfig::new(daemon, forge, role_tokens).with_agent_traces(
            EngineAgentTraceConfig {
                policy: AgentActivityCapturePolicyV1::default(),
                journal_root: Some("/var/lib/temper/agent-traces/journal".into()),
                read_token: Some(SecretString::from("trace-read-super-secret")),
            },
        );

        assert_eq!(config.daemon.daemon_id, "engine-test");
        assert_eq!(config.forge.base_url, "https://forge.example");
        assert!(config.role_tokens.contains_key("coder"));
        assert_eq!(
            config.agent_traces.journal_root.as_deref(),
            Some(std::path::Path::new("/var/lib/temper/agent-traces/journal"))
        );
        let debug = format!("{config:?}");
        assert!(!debug.contains("trace-read-super-secret"), "{debug}");
        assert!(debug.contains("[REDACTED]"), "{debug}");
    }
}
