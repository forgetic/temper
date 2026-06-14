// SPDX-License-Identifier: MPL-2.0

//! `temper provision-forgejo` — provision a Forgejo instance for a workflow.

use std::process::ExitCode;

use temper_forgejo_provision::provision_args::{self, ParseOutcome};

pub fn main<I>(args: I) -> ExitCode
where
    I: Iterator<Item = String>,
{
    match provision_args::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", provision_args::USAGE);
            ExitCode::SUCCESS
        }
        Ok(ParseOutcome::Run(provision_args)) => match provision_args::run(&provision_args) {
            Ok(status) => {
                println!("{status}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("temper-provision-forgejo: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("temper-provision-forgejo: {error}");
            ExitCode::from(2)
        }
    }
}
