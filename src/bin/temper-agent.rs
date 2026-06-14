// SPDX-License-Identifier: MPL-2.0

//! `temper-agent` split binary — a thin wrapper over the shared agent
//! subcommand (the out-of-process coding agent; formerly `anvil-agent`). The
//! same entrypoint is reachable as `temper agent`.

use std::process::ExitCode;

fn main() -> ExitCode {
    temper::cli::agent::main(std::env::args().skip(1))
}
