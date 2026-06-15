// SPDX-License-Identifier: MPL-2.0

//! The agent session's per-subsystem config object.
//!
//! [`AgentConfig`] bundles every knob the coding-agent session is parameterized
//! by, so the session factory takes one struct rather than a long, growing
//! parameter list (the per-subsystem config-object rule — see the crate root
//! docs). It is constructible **in memory** with no files/env/args, so unit and
//! integration tests can build an agent config directly.
//!
//! Today these values still originate from CLI flags and environment reads
//! scattered across the session and the agent core; this struct only *defines*
//! the shape and is what the factory accepts. Relocating the env reads onto these
//! fields is issue #201's job — not this one.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use temper_agent::ProviderConfig;

/// Default agent checkpoint cadence when the host supplies none (mirrors the
/// session's `DEFAULT_CHECKPOINT_INTERVAL`).
pub const DEFAULT_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(300);

/// Everything the coding-agent session is configured by, in one struct.
///
/// Built in `run()` from the parsed options (and, until issue #201 relocates
/// them, the session's existing env reads), or directly in tests. Token-bearing
/// state lives inside [`ProviderConfig`]; this struct adds the session/loop knobs
/// around it.
#[derive(Clone)]
pub struct AgentConfig {
    /// Provider/model/auth wiring (carries any provider credential).
    pub provider: ProviderConfig,
    /// Maximum model turns before the loop gives up.
    pub max_iterations: usize,
    /// Whether the in-workspace `investigate` sub-agent tool is enabled.
    pub enable_subagents: bool,
    /// Optional prompt-overlay config directory (`--config-dir` / `ANVIL_CONFIG_DIR`).
    pub config_dir: Option<PathBuf>,
    /// Optional job deadline the checkpoint backstop respects (the host passes a
    /// unix timestamp via `TEMPER_AGENT_DEADLINE`).
    pub deadline: Option<SystemTime>,
    /// Checkpoint cadence for the time-based backstop.
    pub checkpoint_interval: Duration,
    /// Optional directory the workflow-role decision capture writes to
    /// (`ANVIL_WORKFLOW_ROLE_DECISION_CAPTURE_DIR`).
    pub capture_dir: Option<PathBuf>,
    /// Optional provider base-URL override for hermetic tests
    /// (`ANVIL_TEST_PROVIDER_BASE_URL`).
    pub test_base_url: Option<String>,
}

impl AgentConfig {
    /// Builds an agent config from the provider plus the loop knobs, defaulting
    /// the host-supplied fields (deadline/capture/test base URL) to absent and the
    /// checkpoint interval to [`DEFAULT_CHECKPOINT_INTERVAL`]. The
    /// `with_*`/field setters layer the rest on.
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
            capture_dir: None,
            test_base_url: None,
        }
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
        assert!(config.capture_dir.is_none());
        assert!(config.test_base_url.is_none());
        assert_eq!(config.provider.base_url(), "https://llm.example");
    }
}
