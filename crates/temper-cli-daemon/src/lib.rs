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
//!
//! ## Hermeticity
//!
//! [`run`] takes a [`DaemonInputs`] — explicit `--config` / `--credentials`
//! paths, the `--service`, and the injected env snapshot + base directories the
//! composition root captured. It loads via [`temper_config::load_explicit`]; when
//! an explicit `--config` *or* `--credentials` is given and the env snapshot does
//! not set `TEMPER_CONFIG` / `TEMPER_CREDENTIALS`, default-location discovery is
//! suppressed, so the operator's global `~/.config/temper/credentials.toml` can
//! never ambiently layer in behind an explicit deployment. That layering was the
//! original incident this fixes.

mod provider;
mod standalone;

use std::path::PathBuf;
use std::process::ExitCode;

use temper_config::{
    ConfigError, EX_USAGE, EnvLookup, LoadInputs, LoadedPaths, PathResolver, Resolved,
    load_explicit, parse_common_args,
};

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

/// Which individual daemon service to run (`temper daemon --service <name>`).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Service {
    /// The orchestrator (engine) service.
    Engine,
    /// The orchestration-worker service.
    Worker,
}

impl Service {
    /// Parses a `--service` value, rejecting any other name.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "engine" => Ok(Service::Engine),
            "worker" => Ok(Service::Worker),
            other => Err(format!(
                "unknown --service `{other}` (expected `engine` or `worker`)"
            )),
        }
    }
}

/// Everything `temper daemon` needs, with no ambient environment access.
///
/// `env` / `paths` are the snapshot the composition root (`src/bin`) captured;
/// nothing here reads `std::env`.
pub struct DaemonInputs<'a> {
    /// Explicit `--config` path.
    pub config: Option<PathBuf>,
    /// Explicit `--credentials` path.
    pub credentials: Option<PathBuf>,
    /// Which single service to run, or `None` for the all-in-one standalone.
    pub service: Option<Service>,
    /// The injected environment snapshot (incl. `TEMPER_CONFIG` / `TEMPER_CREDENTIALS`).
    pub env: &'a dyn EnvLookup,
    /// The injected base directories (HOME / XDG_*) for default-location discovery.
    pub paths: &'a PathResolver,
}

/// A failure running the daemon.
#[derive(Debug)]
pub enum DaemonError {
    /// Loading + resolving the config / credentials failed.
    Load(ConfigError),
    /// The selected service (or standalone) failed at runtime.
    Run(String),
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaemonError::Load(error) => write!(f, "{error}"),
            DaemonError::Run(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for DaemonError {}

/// `temper daemon` entry point: parse the common flags + `--service` from
/// `args`, build [`DaemonInputs`] over the injected `env` / `paths` snapshot, and
/// [`run`]. The composition root (`src/bin`) supplies the snapshot; this reads no
/// `std::env`.
pub fn main(args: Vec<String>, env: &dyn EnvLookup, paths: &PathResolver) -> ExitCode {
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

    let inputs = DaemonInputs {
        config: parsed.options.config,
        credentials: parsed.options.credentials,
        service,
        env,
        paths,
    };
    match run(inputs) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper daemon: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Pulls an optional `--service <name>` out of the leftover args, rejecting any
/// other stray argument.
fn extract_service(rest: &[String]) -> Result<Option<Service>, String> {
    let mut service = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--service" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--service requires a value".to_string())?;
                service = Some(Service::parse(value)?);
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(service)
}

/// Loads the deployment from [`DaemonInputs`] and runs the selected service.
///
/// Hermeticity: an explicit `--config` / `--credentials` suppresses default
/// `~/.config/temper` discovery (see [`load_for`]), so the global credentials
/// file never layers in behind an explicit deployment.
pub fn run(inputs: DaemonInputs) -> Result<(), DaemonError> {
    let (resolved, _paths) = load_for(&inputs).map_err(DaemonError::Load)?;
    let result = match inputs.service {
        None => standalone::run(&resolved),
        Some(Service::Engine) => temper_engine_service::run(&resolved),
        Some(Service::Worker) => {
            temper_worker_service::run(&resolved, temper_worker_service::self_subcommand("agent"))
        }
    };
    result.map_err(DaemonError::Run)
}

/// Loads + resolves the deployment from the injected inputs.
///
/// When an explicit `--config` *or* `--credentials` is given, default-location
/// discovery is suppressed by handing the loader an *empty* [`PathResolver`]:
/// only the explicit paths and any `TEMPER_CONFIG` / `TEMPER_CREDENTIALS` in the
/// env snapshot can load. With no explicit path at all, the captured `paths` are
/// used so a plain `temper daemon` still finds `~/.config/temper`.
fn load_for(inputs: &DaemonInputs) -> Result<(Resolved, LoadedPaths), ConfigError> {
    let explicit = inputs.config.is_some() || inputs.credentials.is_some();
    let empty = PathResolver::default();
    let paths: &PathResolver = if explicit { &empty } else { inputs.paths };
    load_explicit(&LoadInputs {
        explicit_config: inputs.config.clone(),
        explicit_credentials: inputs.credentials.clone(),
        env: inputs.env,
        paths,
    })
}

#[cfg(test)]
mod tests;
