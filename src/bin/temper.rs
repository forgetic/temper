// SPDX-License-Identifier: MPL-2.0

//! The unified `temper` binary — a one-line shim over the CLI composition root.
//!
//! All logic lives in `crates/`: this dispatches `config | daemon | agent` (and
//! the hidden operator tools) via [`temper_cli::run`].

use std::process::ExitCode;

fn main() -> ExitCode {
    temper_cli::run()
}
