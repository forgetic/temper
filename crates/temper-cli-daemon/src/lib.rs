// SPDX-License-Identifier: MPL-2.0

//! `temper daemon` / `temper serve` — run the full standalone daemon, or one
//! service of it.
//!
//! `temper daemon` with no `--service` and `temper serve standalone` both run the
//! all-in-one (engine + worker + agent in one process). `temper daemon --service
//! engine`, `temper serve engine`, `temper daemon --service worker`, and
//! `temper serve worker` run a single service for a distributed topology,
//! sharing the exact code the slim `temper-engine` / `temper-worker` binaries
//! run.
//!
//! This crate carries the heavy engine/worker/agent wiring; the slimmer
//! `temper-cli` dispatcher delegates `temper daemon` here and re-exports the
//! reusable in-process transport + in-process agent runner from this crate so
//! existing `temper_cli::{InProcessTransport, InProcessAgentRunner}` users keep
//! the same path.
//!
//! ## Hermeticity
//!
//! [`run`] takes a [`DaemonInputs`] — explicit `--config` / `--secrets` paths,
//! the `--service`, and the injected env snapshot + base directories the
//! composition root captured. It loads via
//! [`temper_config::load_explicit`]; when an explicit `--config` *or* secret path
//! is given, default-location discovery is suppressed. `CREDENTIALS_DIRECTORY`
//! from the injected env may still supply credentials when `--secrets` is absent;
//! otherwise an explicit config root may load its sibling `credentials.toml`, but
//! the operator's global `~/.config/temper/credentials.toml` can never ambiently
//! layer in behind an explicit deployment. That layering was the original
//! incident this fixes.

mod provider;
mod runtime_overrides;
mod serve_args;
mod standalone;

use std::path::PathBuf;
use std::process::ExitCode;

use temper_config::{
    ConfigError, EX_USAGE, EnvLookup, LoadInputs, LoadOptions, LoadedPaths, PathResolver, Resolved,
    load_explicit,
};

pub use runtime_overrides::RuntimeOverrides;
pub(crate) use runtime_overrides::apply_runtime_overrides;
use serve_args::serve_service_usage;
pub use serve_args::{SERVE_ENGINE_USAGE, SERVE_STANDALONE_USAGE, SERVE_USAGE, SERVE_WORKER_USAGE};
pub(crate) use serve_args::{ServeInvocation, parse_serve_invocation};

// Exposed (re-exported up through `temper-cli`) for backwards-compatible
// standalone wiring/tests. The transport implementation itself lives in the
// reusable `temper-daemon-transport` crate, not in the CLI module.
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

    /// Returns the command-line spelling for this service.
    pub fn as_str(self) -> &'static str {
        match self {
            Service::Engine => "engine",
            Service::Worker => "worker",
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
    /// Per-process runtime overrides parsed by `temper serve`. The legacy
    /// `temper daemon` path supplies the default (no overrides).
    pub runtime: RuntimeOverrides,
    /// The injected environment snapshot (used for path expansion and for
    /// systemd `CREDENTIALS_DIRECTORY` credentials discovery).
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
/// `temper serve standalone` maps to the existing standalone daemon path while
/// `temper serve engine|worker` dispatch to the same single-service paths as
/// `temper daemon --service engine|worker`. `temper daemon` stays available as a
/// compatibility surface, and `temper serve trigger` remains unimplemented until
/// its topology is specified.
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
        Ok(ServeInvocation::ServiceHelp(service)) => {
            println!("{}", serve_service_usage(service));
            ExitCode::SUCCESS
        }
        Ok(ServeInvocation::Standalone(runtime)) => {
            serve_standalone_with_options(runtime, env, paths, options)
        }
        Ok(ServeInvocation::Service(service, runtime)) => {
            serve_service_with_options(service, runtime, env, paths, options)
        }
        Err(error) => {
            eprintln!("temper serve: {error}\n\n{SERVE_USAGE}");
            ExitCode::from(EX_USAGE)
        }
    }
}

fn serve_standalone_with_options(
    runtime: RuntimeOverrides,
    env: &dyn EnvLookup,
    paths: &PathResolver,
    options: LoadOptions,
) -> ExitCode {
    let inputs = DaemonInputs {
        config: options.config,
        credentials: options.credentials,
        service: None,
        runtime,
        env,
        paths,
    };
    match run(inputs) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper serve standalone: {error}");
            ExitCode::FAILURE
        }
    }
}

fn serve_service_with_options(
    service: Service,
    runtime: RuntimeOverrides,
    env: &dyn EnvLookup,
    paths: &PathResolver,
    options: LoadOptions,
) -> ExitCode {
    let inputs = DaemonInputs {
        config: options.config,
        credentials: options.credentials,
        service: Some(service),
        runtime,
        env,
        paths,
    };
    match run(inputs) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper serve {}: {error}", service.as_str());
            ExitCode::FAILURE
        }
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
        runtime: RuntimeOverrides::default(),
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
/// `~/.config/temper` discovery (see [`load_for`]). `CREDENTIALS_DIRECTORY` from
/// the injected env may still supply credentials when `--secrets` is absent;
/// otherwise an explicit config root may load sibling `credentials.toml`, but the
/// global credentials file never layers in behind an explicit deployment.
pub fn run(inputs: DaemonInputs) -> Result<(), DaemonError> {
    let (mut resolved, loaded_paths) = load_for(&inputs).map_err(DaemonError::Load)?;
    apply_runtime_overrides(&mut resolved, inputs.service, &inputs.runtime)
        .map_err(DaemonError::Run)?;
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
/// only explicit paths, `CREDENTIALS_DIRECTORY` from the injected env, plus
/// explicit-config sibling credentials can load. With no explicit path at all,
/// the captured `paths` are used so a plain `temper daemon` still finds
/// `~/.config/temper`.
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
