// SPDX-License-Identifier: MPL-2.0

//! `temper interaction` — interactive responder (REPL or serve).

use std::process::ExitCode;

use temper_interaction_service::interaction_args::{self, ParseOutcome};

pub fn main<I>(args: I) -> ExitCode
where
    I: Iterator<Item = String>,
{
    match interaction_args::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", interaction_args::USAGE);
            ExitCode::SUCCESS
        }
        Ok(ParseOutcome::Repl(args)) => {
            if let Err(error) = temper_interaction_service::interaction_repl::run_repl(&args) {
                eprintln!("temper-interaction: {error}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Ok(ParseOutcome::Serve(args)) => {
            if let Err(error) = temper_interaction_service::interaction_serve::run_serve(&args) {
                eprintln!("temper-interaction: {error}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("temper-interaction: {error}");
            ExitCode::from(2)
        }
    }
}
