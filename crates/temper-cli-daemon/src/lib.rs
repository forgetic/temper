// SPDX-License-Identifier: MPL-2.0

//! `temper daemon` / `temper serve standalone` — run the full standalone daemon,
//! or one service of it.
//!
//! `temper daemon` with no `--service` and `temper serve standalone` both run the
//! all-in-one (engine + worker + agent in one process). `temper daemon --service
//! engine` and `temper daemon --service worker` run a single service for a
//! distributed topology, sharing the exact code the slim `temper-engine` /
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
//! [`run`] takes a [`DaemonInputs`] — explicit `--config` / `--secrets` paths,
//! the `--service`, and the injected env snapshot + base directories the
//! composition root captured. It loads via
//! [`temper_config::load_explicit`]; when an explicit `--config` *or* secret path
//! is given, default-location discovery is suppressed. An explicit config root
//! may load its sibling `credentials.toml`, but the operator's global
//! `~/.config/temper/credentials.toml` can never ambiently layer in behind an
//! explicit deployment. That layering was the original incident this fixes.

mod provider;
mod standalone;

use std::path::PathBuf;
use std::process::ExitCode;

use temper_config::{
    ConfigError, EX_USAGE, EnvLookup, LoadInputs, LoadOptions, LoadedPaths, PathResolver, Resolved,
    load_explicit,
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

Use top-level `temper --config ... --secrets ... daemon` to select a deployment
bundle; config files provide runtime settings.

Usage: temper [GLOBAL OPTIONS] daemon [OPTIONS]

Options:
  --service <NAME>  Which individual service to run (engine, worker). If not
                    given, run as standalone.
  -h, --help        Print help";

pub const SERVE_USAGE: &str = "\
Run a Temper process.

Usage: temper [GLOBAL OPTIONS] serve <COMPONENT> [OPTIONS]

Components:
  standalone  Run all Temper planes in one local process
  engine      Not implemented for `temper serve` in this UX shim
  worker      Not implemented for `temper serve` in this UX shim
  trigger     Not implemented for `temper serve` in this UX shim

Options:
  -h, --help  Print help

Run `temper serve standalone --help` for the supported local-dev path.";

pub const SERVE_STANDALONE_USAGE: &str = "\
Run Temper in standalone mode.

This is a compatibility wrapper for the existing all-in-one `temper daemon` path
without `--service`: engine, worker, and agent execution run in one process.

Usage: temper [GLOBAL OPTIONS] serve standalone [OPTIONS]

Options:
  -h, --help  Print help";

#[derive(Debug, Eq, PartialEq)]
enum ServeInvocation {
    Help,
    Version,
    StandaloneHelp,
    Standalone,
}

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
    /// Explicit `--secrets` path.
    pub credentials: Option<PathBuf>,
    /// Which single service to run, or `None` for the all-in-one standalone.
    pub service: Option<Service>,
    /// The injected environment snapshot (used only for `$HOME` / `$XDG_*`
    /// path expansion; no environment variable selects the config files).
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

/// `temper serve` entry point.
///
/// This UX shim intentionally implements only `temper serve standalone`, mapping
/// it to the existing standalone daemon path while keeping `temper daemon`
/// unchanged for compatibility. Distributed `serve engine|worker|trigger` modes
/// are rejected here instead of growing new topology semantics in this PR.
pub fn serve_main(args: Vec<String>, env: &dyn EnvLookup, paths: &PathResolver) -> ExitCode {
    serve_main_with_options(args, env, paths, LoadOptions::default())
}

pub fn serve_main_with_options(
    args: Vec<String>,
    env: &dyn EnvLookup,
    paths: &PathResolver,
    options: LoadOptions,
) -> ExitCode {
    match parse_serve_invocation(args) {
        Ok(ServeInvocation::Help) => {
            println!("{SERVE_USAGE}");
            ExitCode::SUCCESS
        }
        Ok(ServeInvocation::Version) => {
            println!("temper {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(ServeInvocation::StandaloneHelp) => {
            println!("{SERVE_STANDALONE_USAGE}");
            ExitCode::SUCCESS
        }
        Ok(ServeInvocation::Standalone) => main_with_options(Vec::new(), env, paths, options),
        Err(error) => {
            eprintln!("temper serve: {error}\n\n{SERVE_USAGE}");
            ExitCode::from(EX_USAGE)
        }
    }
}

fn parse_serve_invocation(args: Vec<String>) -> Result<ServeInvocation, String> {
    let mut iter = args.into_iter();
    let Some(component) = iter.next() else {
        return Err("missing component (expected `standalone`)".to_string());
    };
    match component.as_str() {
        "-h" | "--help" | "help" => Ok(ServeInvocation::Help),
        "-V" | "--version" => Ok(ServeInvocation::Version),
        "standalone" => parse_serve_standalone(iter.collect()),
        "engine" | "worker" | "trigger" => Err(format!(
            "`temper serve {component}` is not implemented in this UX shim; use `temper serve standalone` for local development"
        )),
        other => Err(format!(
            "unknown serve component `{other}` (expected `standalone`)"
        )),
    }
}

fn parse_serve_standalone(args: Vec<String>) -> Result<ServeInvocation, String> {
    let Some(arg) = args.into_iter().next() else {
        return Ok(ServeInvocation::Standalone);
    };

    match arg.as_str() {
        "-h" | "--help" | "help" => Ok(ServeInvocation::StandaloneHelp),
        "-V" | "--version" => Ok(ServeInvocation::Version),
        "--service" => Err(
            "`temper serve standalone` always runs the standalone path; `--service` is not accepted"
                .to_string(),
        ),
        "-c" | "--config" | "--secrets" => Err(format!(
            "`{arg}` is a global option; place it before `serve`"
        )),
        other => Err(format!("unexpected argument `{other}`")),
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
struct ParsedDaemonArgs {
    help: bool,
    version: bool,
    service: Option<Service>,
}

/// `temper daemon` entry point: parse `--service`, build [`DaemonInputs`] over
/// the injected `env` / `paths` snapshot, and [`run`]. The composition root
/// (`src/bin`) supplies the snapshot; this reads no `std::env`.
pub fn main(args: Vec<String>, env: &dyn EnvLookup, paths: &PathResolver) -> ExitCode {
    main_with_options(args, env, paths, LoadOptions::default())
}

pub fn main_with_options(
    args: Vec<String>,
    env: &dyn EnvLookup,
    paths: &PathResolver,
    options: LoadOptions,
) -> ExitCode {
    let parsed = match parse_daemon_args(args) {
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
    if parsed.version {
        println!("temper {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let inputs = DaemonInputs {
        config: options.config,
        credentials: options.credentials,
        service: parsed.service,
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

fn parse_daemon_args(args: Vec<String>) -> Result<ParsedDaemonArgs, String> {
    let mut parsed = ParsedDaemonArgs::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => parsed.help = true,
            "-V" | "--version" => parsed.version = true,
            "--service" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--service requires a value".to_string())?;
                parsed.service = Some(Service::parse(value)?);
            }
            "-c" | "--config" | "--secrets" => {
                return Err(format!(
                    "`{arg}` is a global option; place it before `daemon`"
                ));
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(parsed)
}

/// Loads the deployment from [`DaemonInputs`] and runs the selected service.
///
/// Hermeticity: an explicit `--config` / `--secrets` suppresses default
/// `~/.config/temper` discovery (see [`load_for`]). An explicit config root may
/// load its sibling `credentials.toml`, but the global credentials file never
/// credentials file never layers in behind an explicit deployment.
pub fn run(inputs: DaemonInputs) -> Result<(), DaemonError> {
    let (resolved, loaded_paths) = load_for(&inputs).map_err(DaemonError::Load)?;
    let result = match inputs.service {
        None => standalone::run(&resolved, loaded_paths.config.as_deref()),
        Some(Service::Engine) => temper_engine_service::run(&resolved),
        Some(Service::Worker) => {
            temper_worker_service::run(&resolved, temper_worker_service::self_subcommand("agent"))
        }
    };
    result.map_err(DaemonError::Run)
}

/// Loads + resolves the deployment from the injected inputs.
///
/// When an explicit `--config` *or* secret path is given, default-location
/// discovery is suppressed by handing the loader an *empty* [`PathResolver`]:
/// only explicit paths plus explicit-config sibling credentials can load. With
/// no explicit path at all, the captured `paths` are used so a plain
/// `temper daemon` still finds `~/.config/temper`.
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
