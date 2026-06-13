// SPDX-License-Identifier: MPL-2.0

//! The `temper` product library: subcommand entrypoints shared by the unified
//! `temper` binary and the role-specific split binaries.
//!
//! Each role/operator subcommand lives in [`cli`] as a `main(args) -> ExitCode`
//! function — the single source of truth for that subcommand. The unified
//! `temper` binary dispatches on `argv[1]`; the split binaries
//! (`temper-daemon`, `temper-worker`, `temper-agent`) are one-line wrappers.
//! Cargo features gate which subcommands compile in, so a worker- or
//! daemon-only build never links the agent's LLM/HTTP stack.

pub mod cli;

/// The unified single-process mode (`temper run`): daemon + worker + agent on
/// one event loop. Available only when both the daemon and worker planes are
/// compiled in (the `unified` profile).
#[cfg(all(feature = "daemon", feature = "worker"))]
pub mod run;
