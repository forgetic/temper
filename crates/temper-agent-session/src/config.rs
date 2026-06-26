// SPDX-License-Identifier: MPL-2.0

//! The agent session's per-subsystem config object.
//!
//! [`AgentConfig`] bundles every knob the coding-agent session is parameterized
//! by, so the session factory takes one struct rather than a long, growing
//! parameter list (the per-subsystem config-object rule — see the crate root
//! docs). It is constructible **in memory** with no files/env/args, so unit and
//! integration tests can build an agent config directly.
//!
//! Every value here originates from the agent process's CLI flags plus the one
//! secret env var (the provider credential), read in exactly one place — the
//! [`entry`](crate::entry) module — and threaded inward through this struct. No
//! code below `entry` reads `std::env`; the deeper modules take their inputs from
//! these fields.
//!
//! The agent no longer carries a git author identity or push token: the worker
//! configures `user.name`/`user.email` and a push credential
//! (`http.extraheader`) in each writable repo's local `.git/config` before
//! spawning the agent, so the checkpointer just runs `git commit`/`git push`
//! against the prepared checkout. The secret push token never reaches the agent.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use temper_agent::ProviderConfig;

/// Default agent checkpoint cadence when the host supplies none (mirrors the
/// session's `DEFAULT_CHECKPOINT_INTERVAL`).
pub const DEFAULT_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(300);

/// Everything the coding-agent session is configured by, in one struct.
///
/// Built in [`entry`](crate::entry) from the parsed CLI options plus the parsed
/// provider credential (read there and nowhere deeper), or directly in tests.
/// Token-bearing state lives inside [`ProviderConfig`]; this struct adds the
/// session/loop knobs around it.
#[derive(Clone)]
pub struct AgentConfig {
    /// Provider/model/auth wiring (carries any provider credential).
    pub provider: ProviderConfig,
    /// Maximum model turns before the loop gives up.
    pub max_iterations: usize,
    /// Whether the in-workspace `investigate` sub-agent tool is enabled.
    pub enable_subagents: bool,
    /// Optional prompt-overlay / debug-capture directory, fully resolved in
    /// `entry` (`--capture-dir`, falling back to `XDG_CONFIG_HOME`/`HOME`).
    pub config_dir: Option<PathBuf>,
    /// Optional job deadline the checkpoint backstop respects (the worker passes
    /// a unix timestamp via `--deadline-unix-seconds`).
    pub deadline: Option<SystemTime>,
    /// Checkpoint cadence for the time-based backstop (`--checkpoint-interval`).
    pub checkpoint_interval: Duration,
    /// Optional daemon endpoint used to revalidate PR-head freshness before
    /// checkpoint pushes.
    pub freshness_url: Option<String>,
}

impl AgentConfig {
    /// Builds an agent config from the provider plus the loop knobs, defaulting
    /// the host-supplied fields (deadline) to absent and the checkpoint interval
    /// to [`DEFAULT_CHECKPOINT_INTERVAL`]. `entry` layers the host-read fields on
    /// with [`with_deadline`](Self::with_deadline) and
    /// [`with_checkpoint_interval`](Self::with_checkpoint_interval).
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
            freshness_url: None,
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

    /// Sets the daemon PR freshness endpoint.
    #[must_use]
    pub fn with_freshness_url(mut self, url: Option<String>) -> Self {
        self.freshness_url = url;
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
        assert_eq!(config.provider.base_url(), "https://llm.example");
    }

    /// The host-read fields layer on through the `with_*` setters.
    #[test]
    fn agent_config_layers_host_fields() {
        use std::time::{Duration, UNIX_EPOCH};

        let provider = ProviderConfig::new("p", "m", "https://llm.example", "k");
        let config = AgentConfig::new(provider, 1, false, None)
            .with_deadline(Some(UNIX_EPOCH + Duration::from_secs(100)))
            .with_checkpoint_interval(Duration::from_secs(42));

        assert_eq!(config.checkpoint_interval, Duration::from_secs(42));
        assert_eq!(config.deadline, Some(UNIX_EPOCH + Duration::from_secs(100)));
    }
}
