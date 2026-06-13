// SPDX-License-Identifier: MPL-2.0

//! The unified `temper` binary: dispatches `argv[1]` to a subcommand.
//!
//! `temper run` is the solo-dev all-in-one; `temper daemon|worker|agent` are the
//! role processes; the rest are operator tools. Which subcommands are present
//! depends on the compiled features (see the crate's `[features]`).

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args();
    let _prog = args.next();
    let Some(subcommand) = args.next() else {
        println!("{}", temper::cli::USAGE);
        return ExitCode::from(temper::cli::EX_USAGE);
    };
    temper::cli::dispatch(&subcommand, args)
}
