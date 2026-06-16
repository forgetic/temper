// SPDX-License-Identifier: MPL-2.0

//! The agent session's per-subsystem config object.
//!
//! [`AgentConfig`] bundles every knob the coding-agent session is parameterized
//! by, so the session factory takes one struct rather than a long, growing
//! parameter list (the per-subsystem config-object rule — see the crate root
//! docs). It is constructible **in memory** with no files/env/args, so unit and
//! integration tests can build an agent config directly.
//!
//! Every value here originates from the agent process's injected environment and
//! CLI flags, read in exactly one place — the [`entry`](crate::entry) module —
//! and threaded inward through this struct. No code below `entry` reads
//! `std::env`; the deeper modules take their inputs from these fields. The struct
//! is constructible **in memory** with no files/env/args, so unit and integration
//! tests can build an agent config directly.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use secrecy::SecretString;
use temper_agent::ProviderConfig;

/// The per-role git identity + push token the checkpointer commits/pushes with.
///
/// Read by [`entry`](crate::entry) from the worker-injected
/// `TEMPER_FORGEJO_{USER,EMAIL,TOKEN}_<ROLE>` env (the `<ROLE>` suffix derives
/// from the work item's role). Every field is optional: absent values degrade
/// gracefully (a generic git identity, a credential-less push).
#[derive(Clone, Default)]
pub struct RoleIdentity {
    /// The git author/committer name (`TEMPER_FORGEJO_USER_<ROLE>`).
    pub user: Option<String>,
    /// The git author/committer email (`TEMPER_FORGEJO_EMAIL_<ROLE>`).
    pub email: Option<String>,
    /// The push token (`TEMPER_FORGEJO_TOKEN_<ROLE>`); secret.
    pub token: Option<SecretString>,
}

/// Default agent checkpoint cadence when the host supplies none (mirrors the
/// session's `DEFAULT_CHECKPOINT_INTERVAL`).
pub const DEFAULT_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(300);

/// Everything the coding-agent session is configured by, in one struct.
///
/// Built in [`entry`](crate::entry) from the parsed CLI options plus the
/// injected environment (read there and nowhere deeper), or directly in tests.
/// Token-bearing state lives inside [`ProviderConfig`] and [`RoleIdentity`]; this
/// struct adds the session/loop knobs around it.
#[derive(Clone)]
pub struct AgentConfig {
    /// Provider/model/auth wiring (carries any provider credential).
    pub provider: ProviderConfig,
    /// Maximum model turns before the loop gives up.
    pub max_iterations: usize,
    /// Whether the in-workspace `investigate` sub-agent tool is enabled.
    pub enable_subagents: bool,
    /// Optional prompt-overlay config directory, fully resolved in `entry`
    /// (`--config-dir` / `ANVIL_CONFIG_DIR` / `XDG_CONFIG_HOME` / `HOME`).
    pub config_dir: Option<PathBuf>,
    /// Optional job deadline the checkpoint backstop respects (the host passes a
    /// unix timestamp via `TEMPER_AGENT_DEADLINE`).
    pub deadline: Option<SystemTime>,
    /// Checkpoint cadence for the time-based backstop
    /// (`TEMPER_AGENT_CHECKPOINT_INTERVAL_SECS`).
    pub checkpoint_interval: Duration,
    /// The per-role git identity + push token for checkpoint commits/pushes.
    pub role_identity: RoleIdentity,
}

impl AgentConfig {
    /// Builds an agent config from the provider plus the loop knobs, defaulting
    /// the host-supplied fields (deadline, role identity) to absent and the
    /// checkpoint interval to [`DEFAULT_CHECKPOINT_INTERVAL`]. `entry` layers the
    /// host-read fields on with [`with_deadline`](Self::with_deadline),
    /// [`with_checkpoint_interval`](Self::with_checkpoint_interval), and
    /// [`with_role_identity`](Self::with_role_identity).
    pub fn new(
        provider: ProviderConfig,
        max_iterations: usize,
        enable_subagents: bool,
        config_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            provider,
            max_iterations,
            enable_subagents,
            config_dir,
            deadline: None,
            checkpoint_interval: DEFAULT_CHECKPOINT_INTERVAL,
            role_identity: RoleIdentity::default(),
        }
    }

    /// Sets the job deadline (lease expiry) the checkpoint backstop respects.
    #[must_use]
    pub fn with_deadline(mut self, deadline: Option<SystemTime>) -> Self {
        self.deadline = deadline;
        self
    }

    /// Sets the checkpoint backstop cadence (defaults to
    /// [`DEFAULT_CHECKPOINT_INTERVAL`]).
    #[must_use]
    pub fn with_checkpoint_interval(mut self, interval: Duration) -> Self {
        self.checkpoint_interval = interval;
        self
    }

    /// Sets the per-role git identity + push token for checkpoint commits.
    #[must_use]
    pub fn with_role_identity(mut self, role_identity: RoleIdentity) -> Self {
        self.role_identity = role_identity;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The agent config is constructible entirely in memory — no files, env, or
    /// args — so a subsystem test can build the session's inputs directly.
    #[test]
    fn agent_config_builds_in_memory() {
        let provider = ProviderConfig::new(
            "test-provider",
            "test-model",
            "https://llm.example",
            "test-key",
        );
        let config = AgentConfig::new(provider, 42, true, Some(PathBuf::from("/cfg")));

        assert_eq!(config.max_iterations, 42);
        assert!(config.enable_subagents);
        assert_eq!(config.config_dir, Some(PathBuf::from("/cfg")));
        assert_eq!(config.checkpoint_interval, DEFAULT_CHECKPOINT_INTERVAL);
        assert!(config.deadline.is_none());
        assert!(config.role_identity.user.is_none());
        assert!(config.role_identity.token.is_none());
        assert_eq!(config.provider.base_url(), "https://llm.example");
    }

    /// The host-read fields layer on through the `with_*` setters.
    #[test]
    fn agent_config_layers_host_fields() {
        use secrecy::ExposeSecret;
        use std::time::{Duration, UNIX_EPOCH};

        let provider = ProviderConfig::new("p", "m", "https://llm.example", "k");
        let config = AgentConfig::new(provider, 1, false, None)
            .with_deadline(Some(UNIX_EPOCH + Duration::from_secs(100)))
            .with_checkpoint_interval(Duration::from_secs(42))
            .with_role_identity(RoleIdentity {
                user: Some("bot".to_string()),
                email: Some("bot@example.com".to_string()),
                token: Some(SecretString::from("push-token".to_string())),
            });

        assert_eq!(config.checkpoint_interval, Duration::from_secs(42));
        assert_eq!(config.deadline, Some(UNIX_EPOCH + Duration::from_secs(100)));
        assert_eq!(config.role_identity.user.as_deref(), Some("bot"));
        assert_eq!(
            config
                .role_identity
                .token
                .as_ref()
                .map(ExposeSecret::expose_secret),
            Some("push-token")
        );
    }
}
