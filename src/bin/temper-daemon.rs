// SPDX-License-Identifier: MPL-2.0

//! `temper-daemon` split binary — a thin wrapper over the shared daemon
//! subcommand. The same entrypoint is reachable as `temper daemon`.

use std::process::ExitCode;

fn main() -> ExitCode {
    temper::cli::daemon::main(std::env::args().skip(1))
}
