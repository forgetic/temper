// SPDX-License-Identifier: MPL-2.0

//! `temper-agent` — the out-of-process coding agent the orchestration worker spawns.
//!
//! This is the worker ↔ agent process boundary (plane 1), reachable both as the
//! slim `temper-agent` binary and as the unified `temper agent` subcommand. It
//! speaks the `temper-protocol-agent`:
//!
//! 1. read the [`WorkspaceContext`] JSON from the file named by `--context`;
//! 2. run the native sans-IO coding loop in `--workspace` (default cwd — the
//!    prepared checkout the worker handed us);
//! 3. write the [`WorkspaceResult`] JSON to the file named by `--result`.
//!
//! The agent has git credentials only via the prepared checkout (the worker
//! configures `user.name`/`user.email` and a push `http.extraheader` in each
//! writable repo's local `.git/config` before spawning), and never talks to the
//! forge API. The worker owns the final branch push and all Forge mutations.
//! Anything real-time (token deltas, steering) belongs to the out-of-band
//! control plane, not this binary's stdout.
//!
//! Every non-secret input is a flag: `--provider <anthropic|chatgpt|deepseek>`,
//! `--model <id>`, `--investigate-model <id>`, `--provider-url <url>`,
//! `--max-iterations <n>`, `--subagents <on|off>`, `--capture-dir <dir>`,
//! optional non-secret `--tool-config <file>`, the optional worker-owned
//! `--submit-for-pr-address <addr>` side channel, plus the required
//! `--context`/`--result` paths and the optional `--workspace`. The **one**
//! secret, the provider credential, arrives via
//! `TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON`.
//!
//! ## Config objects
//!
//! The agent session is configured by one struct, [`AgentConfig`], which the
//! coding-loop factory accepts. It bundles the provider wiring plus every
//! loop/session knob (iterations, sub-agents, capture dir, stored tool config)
//! so the factory takes a single struct rather than a growing parameter list. It
//! is constructible in memory for tests. Per the per-subsystem config-object
//! rule, big factories take a config object; small ones stay as-is.
//!
//! [`WorkspaceContext`]: temper_protocol_agent::WorkspaceContext
//! [`WorkspaceResult`]: temper_protocol_agent::WorkspaceResult

mod config;
mod entry;
mod options;
mod run;
mod submit_client;

use std::process::ExitCode;

pub use config::AgentConfig;

/// The agent binary's entry point: the **single place** this crate (and the
/// `temper-agent` core it drives) reads `std::env`. It reads the one secret env
/// var (the provider credential) and parses the CLI flags into an [`AgentConfig`]
/// plus the context/result/workspace paths, then drives the protocol run;
/// nothing deeper touches `std::env`.
pub fn main<I>(args: I) -> ExitCode
where
    I: Iterator<Item = String>,
{
    match entry::run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // stderr carries diagnostics; stdout is reserved for the result
            // protocol carrier's parent process.
            eprintln!("temper-agent: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    #[test]
    fn help_exits_successfully_without_agent_run_inputs() {
        assert_eq!(
            crate::main(vec!["--help".to_string()].into_iter()),
            ExitCode::SUCCESS
        );
        assert_eq!(
            crate::main(vec!["-h".to_string()].into_iter()),
            ExitCode::SUCCESS
        );
    }
}
