// SPDX-License-Identifier: MPL-2.0

//! The unified `temper` binary — the composition root.
//!
//! This is one of the *only* places that reads the real process environment:
//! [`boot`] snapshots `argv`, the environment (`EnvMap::from_system`), the base
//! directories (`PathResolver::from_system`), and the cwd into a [`CliEnv`], then
//! hands that plain-data snapshot to [`temper_cli::run`]. Every subcommand works
//! off the snapshot, so no library code touches `std::env` (enforced by
//! `scripts/check-no-ambient-env.sh`). All logic lives in `crates/`.

use std::process::ExitCode;

use temper_cli::CliEnv;
use temper_config::{EnvMap, PathResolver};

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    if let Some(status) = temper_cli::dispatch_linux_supervisor_helper(std::env::args_os().skip(1))
    {
        return status;
    }

    temper_cli::run(boot())
}

/// Snapshots the real process environment into a [`CliEnv`].
///
/// The single env-reading boundary for the unified binary: `argv`, the
/// environment, the HOME/XDG base directories, and the cwd are captured once
/// here and threaded down. Everything below takes the resulting plain data.
fn boot() -> CliEnv {
    CliEnv {
        args: std::env::args().skip(1).collect(),
        env: EnvMap::from_system(),
        paths: PathResolver::from_system(),
        cwd: std::env::current_dir().unwrap_or_default(),
    }
}
