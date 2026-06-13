// SPDX-License-Identifier: MPL-2.0

//! `temper-worker` split binary — a thin wrapper over the shared worker
//! subcommand (the orchestration worker; formerly `smith-worker`). The same
//! entrypoint is reachable as `temper worker`.

use std::process::ExitCode;

fn main() -> ExitCode {
    temper::cli::worker::main(std::env::args().skip(1))
}
