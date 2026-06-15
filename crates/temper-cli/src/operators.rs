// SPDX-License-Identifier: MPL-2.0

//! Hidden operator subcommands — thin wrappers over the operator crates. Kept
//! dispatchable (for provisioning, e2e tests, and the interaction service) but
//! out of the headline `temper --help`.

use std::process::ExitCode;

/// `temper provision-forgejo` — provision a Forgejo instance for a workflow.
pub fn provision_forgejo(args: std::env::Args) -> ExitCode {
    use temper_forgejo_provision::provision_args::{self, ParseOutcome};
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
                eprintln!("temper provision-forgejo: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("temper provision-forgejo: {error}");
            ExitCode::from(2)
        }
    }
}

/// `temper trigger-forgejo` — trigger a Forgejo role feed.
pub fn trigger_forgejo(args: std::env::Args) -> ExitCode {
    use temper_trigger_forgejo::trigger_args::{self, ParseOutcome};
    match trigger_args::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", trigger_args::USAGE);
            ExitCode::SUCCESS
        }
        Ok(ParseOutcome::Run(trigger_args)) => {
            if let Err(error) = trigger_args::run(&trigger_args) {
                eprintln!("temper trigger-forgejo: {error}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("temper trigger-forgejo: {error}");
            ExitCode::from(2)
        }
    }
}

/// `temper validate-reference-delivery` — validate a reference-delivery run.
pub fn validate_reference_delivery(args: std::env::Args) -> ExitCode {
    use temper_reference_delivery_validator::reference_delivery_validator::{
        self, ParseOutcome, RunError,
    };
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
                eprintln!("temper validate-reference-delivery: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("temper validate-reference-delivery: {error}");
            ExitCode::from(2)
        }
    }
}

/// `temper interaction` — interactive responder (REPL or serve).
pub fn interaction(args: std::env::Args) -> ExitCode {
    use temper_interaction_service::interaction_args::{self, ParseOutcome};
    match interaction_args::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", interaction_args::USAGE);
            ExitCode::SUCCESS
        }
        Ok(ParseOutcome::Repl(args)) => {
            if let Err(error) = temper_interaction_service::interaction_repl::run_repl(&args) {
                eprintln!("temper interaction: {error}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Ok(ParseOutcome::Serve(args)) => {
            if let Err(error) = temper_interaction_service::interaction_serve::run_serve(&args) {
                eprintln!("temper interaction: {error}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("temper interaction: {error}");
            ExitCode::from(2)
        }
    }
}
