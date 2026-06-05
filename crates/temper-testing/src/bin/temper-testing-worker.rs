//! A single fake worker process for the multi-process end-to-end rehearsal.
//!
//! The multi-process test (Phase 4) spawns this binary once per moving part —
//! each role worker, the mechanical worker, and the fake CI producer — all
//! sharing one `FilesystemForge` store and coordinating only through it. The
//! dispatch and run logic live in `temper_testing::worker_bin` so the default
//! suite can exercise the wiring without spawning a process.

use std::process::ExitCode;
use temper_testing::worker_bin::{self, ParseOutcome};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match worker_bin::parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {}", worker_bin::USAGE);
            ExitCode::SUCCESS
        }
        Ok(ParseOutcome::Run(worker_args)) => match worker_bin::run(&worker_args) {
            Ok(_) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("temper-testing-worker: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("temper-testing-worker: {error}");
            ExitCode::FAILURE
        }
    }
}
