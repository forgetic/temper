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
//! 3. emit [`StepProgress`] records as line-delimited JSON on **stdout** — each
//!    a crash-recovery checkpoint the worker relays to the forge. On writable
//!    jobs the run **checkpoints**: before each model turn it commits + pushes
//!    whatever the previous turn changed and emits a marker carrying the
//!    pushed sha (the marker never claims more than the branch holds). A
//!    re-dispatched agent finds those commits on the prepared branch, resumes
//!    its step numbering from them, and tells the model what already landed;
//! 4. write the [`WorkspaceResult`] JSON to the file named by `--result`.
//!
//! The agent has git credentials only via the prepared checkout (the worker
//! configures `user.name`/`user.email` and a push `http.extraheader` in each
//! writable repo's local `.git/config` before spawning), and never talks to the
//! forge API — the worker owns that. Anything real-time (token deltas, steering)
//! belongs to the out-of-band control plane, not this binary's stdout.
//!
//! Every non-secret input is a flag: `--provider <anthropic|chatgpt|deepseek>`,
//! `--model <id>`, `--investigate-model <id>`, `--provider-url <url>`,
//! `--max-iterations <n>`, `--subagents <on|off>`, `--deadline-unix-seconds <n>`,
//! `--checkpoint-interval <dur>`, `--capture-dir <dir>`, plus the required
//! `--context`/`--result` paths and the optional `--workspace`. The **one**
//! secret, the provider credential, arrives via
//! `TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON`.
//!
//! ## Config objects
//!
//! The agent session is configured by one struct, [`AgentConfig`], which the
//! coding-loop factory accepts. It bundles the provider wiring plus every
//! loop/session knob (iterations, sub-agents, capture dir, deadline, checkpoint
//! cadence) so the factory takes a single struct rather than a growing parameter
//! list. It is constructible in memory for tests. Per the per-subsystem
//! config-object rule, big factories take a config object; small ones stay as-is.
//!
//! [`WorkspaceContext`]: temper_protocol_agent::WorkspaceContext
//! [`StepProgress`]: temper_protocol_agent::StepProgress
//! [`WorkspaceResult`]: temper_protocol_agent::WorkspaceResult

mod checkpoint;
mod config;
mod entry;
mod options;
mod progress;
mod run;

use std::process::ExitCode;

pub use config::{AgentConfig, DEFAULT_CHECKPOINT_INTERVAL};

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
            // stderr carries diagnostics; stdout is reserved for the framed
            // step-progress stream so the worker can parse it cleanly.
            eprintln!("temper-agent: {error}");
            ExitCode::from(2)
        }
    }
}
