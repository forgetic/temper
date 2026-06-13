// SPDX-License-Identifier: MPL-2.0

//! `temper validate-reference-delivery` — validate a reference-delivery run.

use std::process::ExitCode;

use temper_reference_delivery_validator::reference_delivery_validator::{
    self, ParseOutcome, RunError,
};

pub fn main<I>(args: I) -> ExitCode
where
    I: Iterator<Item = String>,
{
    match reference_delivery_validator::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", reference_delivery_validator::USAGE);
            ExitCode::SUCCESS
        }
        Ok(ParseOutcome::Run(args)) => match reference_delivery_validator::run(&args) {
            Ok(output) => {
                println!("{output}");
                ExitCode::SUCCESS
            }
            Err(RunError::ValidationFailed(output)) => {
                eprintln!("{output}");
                ExitCode::FAILURE
            }
            Err(error) => {
                eprintln!("temper-validate-reference-delivery: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("temper-validate-reference-delivery: {error}");
            ExitCode::from(2)
        }
    }
}
