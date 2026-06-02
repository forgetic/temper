//! Operator provisioning + seed entry point for the real-world example.
//!
//! Given a running Forgejo and an admin token (via `TEMPER_FORGEJO_ADMIN_TOKEN`,
//! never argv), this provisions the org/users/tokens/repo/labels/CI workflow,
//! seeds one intake issue, and writes the per-role secrets to the `--out` file
//! the launch script sources. The reusable parse/run logic lives in
//! `temper_testing::provision_bin`; this binary is a thin entry point.

use std::process::ExitCode;
use temper_testing::provision_bin::{self, ParseOutcome};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match provision_bin::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", provision_bin::USAGE);
            ExitCode::SUCCESS
        }
        Ok(ParseOutcome::Run(args)) => match provision_bin::run(&args) {
            Ok(status) => {
                println!("{status}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("temper-testing-provision: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("temper-testing-provision: {error}");
            ExitCode::FAILURE
        }
    }
}
