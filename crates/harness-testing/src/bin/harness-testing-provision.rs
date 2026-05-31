//! Operator provisioning + seed entry point for the real-world example.
//!
//! Given a running Forgejo and an admin token (via `HARNESS_FORGEJO_ADMIN_TOKEN`,
//! never argv), this provisions the org/users/tokens/repo/labels/CI workflow,
//! seeds one intake issue, and writes the per-role secrets to the `--out` file
//! the launch script sources. The reusable parse/run logic lives in
//! `harness_testing::provision_bin`; this binary is a thin entry point.

use harness_testing::provision_bin::{self, ParseOutcome};
use std::process::ExitCode;

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
                eprintln!("harness-testing-provision: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("harness-testing-provision: {error}");
            ExitCode::FAILURE
        }
    }
}
