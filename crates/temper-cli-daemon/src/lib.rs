// SPDX-License-Identifier: MPL-2.0

//! `temper daemon` — run the full standalone daemon, or one service of it.
//!
//! With no `--service`, runs the all-in-one (engine + worker + agent in one
//! process). `--service engine` and `--service worker` run a single service for
//! a distributed topology, sharing the exact code the slim `temper-engine` /
//! `temper-worker` binaries run.
//!
//! This crate carries the heavy engine/worker/agent wiring; the slimmer
//! `temper-cli` dispatcher delegates `temper daemon` here and re-exports the
//! in-process transport + agent runner from this crate so the root integration
//! test keeps the same `temper_cli::{InProcessTransport, InProcessAgentRunner}`
//! path.

mod provider;
mod standalone;

use std::process::ExitCode;

use temper_config::{EX_USAGE, load, parse_common_args};

// Exposed (re-exported up through `temper-cli`) for the root package's
// in-process-transport integration test, which proves the standalone
// worker→daemon carrier in isolation.
pub use standalone::{InProcessAgentRunner, InProcessTransport};

pub const USAGE: &str = "\
Run the temper daemon.

The temper daemon can be run as:

- Standalone: all services run in one process (engine, workers, agents, etc.)
- Distributed topology: individual services run as scalable separate processes

Command line flags override env vars, env vars override config file, config file
override defaults.

Usage: temper daemon [OPTIONS]

Options:
  --config      <FILE>  Path to configuration file
  --credentials <FILE>  Secrets for external services (Forgejo, LLM providers, ...)
  --service     <NAME>  Which individual service to run (engine, worker). If not
                        given, run as standalone.
  -h, --help            Print help";

pub fn main(args: std::env::Args) -> ExitCode {
    let parsed = match parse_common_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("temper daemon: {error}\n\n{USAGE}");
            return ExitCode::from(EX_USAGE);
        }
    };
    if parsed.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let service = match extract_service(&parsed.rest) {
        Ok(service) => service,
        Err(error) => {
            eprintln!("temper daemon: {error}\n\n{USAGE}");
            return ExitCode::from(EX_USAGE);
        }
    };

    let (resolved, _paths) = match load(&parsed.options) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("temper daemon: {error}");
            return ExitCode::FAILURE;
        }
    };

    let result = match service.as_deref() {
        None => standalone::run(&resolved),
        Some("engine") => temper_engine_service::run(&resolved),
        Some("worker") => {
            temper_worker_service::run(&resolved, temper_worker_service::self_subcommand("agent"))
        }
        Some(other) => Err(format!(
            "unknown --service `{other}` (expected `engine` or `worker`)"
        )),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper daemon: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Pulls an optional `--service <name>` out of the leftover args, rejecting any
/// other stray argument.
fn extract_service(rest: &[String]) -> Result<Option<String>, String> {
    let mut service = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--service" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--service requires a value".to_string())?;
                service = Some(value.clone());
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(service)
}
