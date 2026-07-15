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

use std::path::PathBuf;

use temper_agent::{ForgeContextHost, ProviderConfig, SubmitForPrHost};
use temper_protocol_activity::AgentActivityCapturePolicyV1;
use temper_protocol_agent::AgentToolConfig;

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
    /// Optional non-secret tool settings supplied by the worker via
    /// `--tool-config`. The native coding loop builds the codebase-memory MCP
    /// bridge from this config when it applies to the current role.
    pub tool_config: Option<AgentToolConfig>,
    /// Worker-resolved shared trace capture policy. The activity producer
    /// consumes this in the agent tier; it contains no storage path or token.
    pub trace_policy: AgentActivityCapturePolicyV1,
    /// Optional worker-owned local endpoint for newline-delimited activity
    /// frames. Absence preserves legacy/third-party behavior.
    pub activity_address: Option<String>,
    /// Optional host submit callback. In out-of-process mode this is a thin
    /// client for the worker-owned local side channel; when absent the
    /// `submit_for_pr` tool is not exposed by this agent process.
    pub submit_for_pr: Option<SubmitForPrHost>,
    /// Optional asynchronous host channel for bounded read-only Forge context.
    pub forge_context: Option<ForgeContextHost>,
}

impl AgentConfig {
    /// Builds an agent config from the provider plus the loop knobs.
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
            tool_config: None,
            trace_policy: AgentActivityCapturePolicyV1::default(),
            activity_address: None,
            submit_for_pr: None,
            forge_context: None,
        }
    }

    /// Stores the parsed non-secret agent tool config for this session.
    pub fn with_tool_config(mut self, tool_config: Option<AgentToolConfig>) -> Self {
        self.tool_config = tool_config;
        self
    }

    /// Stores the worker-resolved capture policy for this session.
    pub fn with_trace_policy(mut self, trace_policy: AgentActivityCapturePolicyV1) -> Self {
        self.trace_policy = trace_policy;
        self
    }

    /// Installs the optional worker-owned activity endpoint.
    pub fn with_activity_address(mut self, activity_address: Option<String>) -> Self {
        self.activity_address = activity_address;
        self
    }

    /// Installs the host submit callback for this session.
    pub fn with_submit_for_pr(mut self, submit_for_pr: SubmitForPrHost) -> Self {
        self.submit_for_pr = Some(submit_for_pr);
        self
    }

    /// Installs the asynchronous Forge context callback for this session.
    pub fn with_forge_context(mut self, forge_context: ForgeContextHost) -> Self {
        self.forge_context = Some(forge_context);
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
        assert!(config.tool_config.is_none());
        assert_eq!(config.trace_policy, AgentActivityCapturePolicyV1::default());
        assert!(config.activity_address.is_none());
        assert_eq!(config.provider.base_url(), "https://llm.example");
    }
}
