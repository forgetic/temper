//! Deterministic wire-protocol test worker for the daemon-topology e2e tests.
//!
//! See `temper_testing::daemon_worker` for the behavior; this binary only
//! parses argv, reads the git identity from the environment, and drives the
//! worker loop on a current-thread runtime.

use std::process::ExitCode;

use temper_testing::daemon_worker::{parse_args, run, GitIdentity, ParseOutcome, USAGE};

fn main() -> ExitCode {
    let config = match parse_args(std::env::args().skip(1)) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(ParseOutcome::Run(config)) => config,
        Err(error) => {
            eprintln!("temper-testing-daemon-worker: {error}\nusage: {USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let identity = match GitIdentity::from_env() {
        Ok(identity) => identity,
        Err(error) => {
            eprintln!("temper-testing-daemon-worker: {error}\nusage: {USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let runtime = match temper_io_engine::build_runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("temper-testing-daemon-worker: failed to build engine runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    match temper_io_engine::runtime::block_on_runtime(&runtime, async move {
        run(&config, &identity).await
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper-testing-daemon-worker: {error}");
            ExitCode::FAILURE
        }
    }
}
