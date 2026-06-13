// SPDX-License-Identifier: MPL-2.0

//! `temper trigger-forgejo` — trigger a Forgejo role feed.

use std::process::ExitCode;

use temper_trigger_forgejo::trigger_args::{self, ParseOutcome};

pub fn main<I>(args: I) -> ExitCode
where
    I: Iterator<Item = String>,
{
    match trigger_args::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", trigger_args::USAGE);
            ExitCode::SUCCESS
        }
        Ok(ParseOutcome::Run(trigger_args)) => {
            if let Err(error) = trigger_args::run(&trigger_args) {
                eprintln!("temper-trigger-forgejo: {error}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("temper-trigger-forgejo: {error}");
            ExitCode::from(2)
        }
    }
}
