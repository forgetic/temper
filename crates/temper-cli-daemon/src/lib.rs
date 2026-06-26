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
//! is given, default-location discovery is suppressed. `CREDENTIALS_DIRECTORY`
//! from the injected env may still supply credentials when `--secrets` is absent;
//! otherwise an explicit config root may load its sibling `credentials.toml`, but
//! the operator's global `~/.config/temper/credentials.toml` can never ambiently
//! layer in behind an explicit deployment. That layering was the original
//! incident this fixes.

mod provider;
mod standalone;

use std::path::PathBuf;
use std::process::ExitCode;

use temper_config::{
    Capability, ConfigError, EX_USAGE, EnvLookup, LoadInputs, LoadOptions, LoadedPaths,
    PathResolver, Resolved, WorkerPoolSettings, load_explicit,
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
  engine      Run the engine service (`temper daemon --service engine`)
  worker      Run the worker service (`temper daemon --service worker`)
  trigger     Not implemented yet for `temper serve`

Options:
  -h, --help  Print help

Component options:
  standalone  --id <ID>
  engine      --id <ID>
  worker      --pool <NAME> --id <ID> --capacity <N> --engine-url <URL>

Place `--config` / `--secrets` before `serve`, for example:
  temper --config ./deploy --secrets ./deploy/credentials.toml serve engine

Run `temper serve <component> --help` for component-specific usage.";

pub const SERVE_STANDALONE_USAGE: &str = "\
Run Temper in standalone mode.

This is a compatibility wrapper for the existing all-in-one `temper daemon` path
without `--service`: engine, worker, and agent execution run in one process.

Usage: temper [GLOBAL OPTIONS] serve standalone [--id <ID>]

Options:
      --id <ID>  Stable process identity for the all-in-one runtime
  -h, --help    Print help";

pub const SERVE_ENGINE_USAGE: &str = "\
Run the Temper engine service.

This is a compatibility wrapper for the existing `temper daemon --service engine`
path. Put deployment file flags before `serve`:
  temper --config ./deploy --secrets ./deploy/credentials.toml serve engine

Usage: temper [GLOBAL OPTIONS] serve engine [--id <ID>]

Options:
      --id <ID>  Override the engine daemon/process identity
  -h, --help    Print help";

pub const SERVE_WORKER_USAGE: &str = "\
Run the Temper worker service.

This is a compatibility wrapper for the existing `temper daemon --service worker`
path. Put deployment file flags before `serve`:
  temper --config ./deploy --secrets ./deploy/credentials.toml serve worker

Usage: temper [GLOBAL OPTIONS] serve worker [OPTIONS]

Options:
      --pool <NAME>       Select a resolved [[worker.pools]] capability class
      --id <ID>           Override the worker registration/logging identity
      --capacity <N>      Override max concurrent jobs for this worker (N > 0)
      --engine-url <URL>  Override the engine/daemon URL for this worker
  -h, --help             Print help";

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct RuntimeOverrides {
    /// Stable process identity override. For standalone this is applied to both
    /// the in-process engine daemon id and worker id; for single-service modes it
    /// applies to the selected service identity.
    pub process_id: Option<String>,
    /// Selected target-era worker pool name (`temper serve worker --pool`).
    pub worker_pool: Option<String>,
    /// Per-process worker capacity override (`temper serve worker --capacity`).
    pub worker_capacity: Option<u32>,
    /// Per-process worker daemon/engine URL override (`temper serve worker --engine-url`).
    pub worker_engine_url: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
enum ServeInvocation {
    Help,
    Version,
    StandaloneHelp,
    ServiceHelp(Service),
    Standalone(RuntimeOverrides),
    Service(Service, RuntimeOverrides),
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

fn serve_service_usage(service: Service) -> &'static str {
    match service {
        Service::Engine => SERVE_ENGINE_USAGE,
        Service::Worker => SERVE_WORKER_USAGE,
    }
}

fn parse_serve_invocation(args: Vec<String>) -> Result<ServeInvocation, String> {
    let mut iter = args.into_iter();
    let Some(component) = iter.next() else {
        return Err("missing component (expected `standalone`, `engine`, or `worker`)".to_string());
    };
    match component.as_str() {
        "-h" | "--help" | "help" => Ok(ServeInvocation::Help),
        "-V" | "--version" => Ok(ServeInvocation::Version),
        "standalone" => parse_serve_standalone(iter.collect()),
        "engine" => parse_serve_service(Service::Engine, iter.collect()),
        "worker" => parse_serve_service(Service::Worker, iter.collect()),
        "trigger" => Err(
            "`temper serve trigger` is not implemented yet; trigger support remains a later workitem"
                .to_string(),
        ),
        other => Err(format!(
            "unknown serve component `{other}` (expected `standalone`, `engine`, or `worker`)"
        )),
    }
}

fn parse_serve_standalone(args: Vec<String>) -> Result<ServeInvocation, String> {
    match parse_serve_component_args(ServeComponent::Standalone, args)? {
        ServeComponentAction::Help => Ok(ServeInvocation::StandaloneHelp),
        ServeComponentAction::Version => Ok(ServeInvocation::Version),
        ServeComponentAction::Run(runtime) => Ok(ServeInvocation::Standalone(runtime)),
    }
}

fn parse_serve_service(service: Service, args: Vec<String>) -> Result<ServeInvocation, String> {
    let component = match service {
        Service::Engine => ServeComponent::Engine,
        Service::Worker => ServeComponent::Worker,
    };
    match parse_serve_component_args(component, args)? {
        ServeComponentAction::Help => Ok(ServeInvocation::ServiceHelp(service)),
        ServeComponentAction::Version => Ok(ServeInvocation::Version),
        ServeComponentAction::Run(runtime) => Ok(ServeInvocation::Service(service, runtime)),
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ServeComponent {
    Standalone,
    Engine,
    Worker,
}

impl ServeComponent {
    fn as_str(self) -> &'static str {
        match self {
            ServeComponent::Standalone => "standalone",
            ServeComponent::Engine => "engine",
            ServeComponent::Worker => "worker",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ServeComponentAction {
    Help,
    Version,
    Run(RuntimeOverrides),
}

fn parse_serve_component_args(
    component: ServeComponent,
    args: Vec<String>,
) -> Result<ServeComponentAction, String> {
    let mut runtime = RuntimeOverrides::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" | "help" => {
                reject_remaining_serve_args(component, &arg, iter.next())?;
                return Ok(ServeComponentAction::Help);
            }
            "-V" | "--version" => {
                reject_remaining_serve_args(component, &arg, iter.next())?;
                return Ok(ServeComponentAction::Version);
            }
            "--service" => return Err(service_flag_error(component)),
            "-c" | "--config" | "--secrets" => {
                return Err(format!("`{arg}` is a global option; place it before `serve`"));
            }
            "--id" => {
                runtime.process_id = Some(next_non_empty_serve_value(component, &mut iter, &arg)?);
            }
            "--pool" => {
                require_worker_flag(component, &arg)?;
                runtime.worker_pool = Some(next_non_empty_serve_value(component, &mut iter, &arg)?);
            }
            "--capacity" => {
                require_worker_flag(component, &arg)?;
                let raw = next_serve_value(component, &mut iter, &arg)?;
                runtime.worker_capacity = Some(parse_serve_capacity(&raw)?);
            }
            "--engine-url" => {
                require_worker_flag(component, &arg)?;
                runtime.worker_engine_url =
                    Some(next_non_empty_serve_value(component, &mut iter, &arg)?);
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }

    Ok(ServeComponentAction::Run(runtime))
}

fn reject_remaining_serve_args(
    component: ServeComponent,
    arg: &str,
    extra: Option<String>,
) -> Result<(), String> {
    if let Some(extra) = extra {
        Err(format!(
            "unexpected argument `{extra}` after `{arg}` for `temper serve {}`",
            component.as_str()
        ))
    } else {
        Ok(())
    }
}

fn service_flag_error(component: ServeComponent) -> String {
    if component == ServeComponent::Standalone {
        "`temper serve standalone` always runs the standalone path; `--service` is not accepted"
            .to_string()
    } else {
        format!(
            "`temper serve {}` already selects the {} service; `--service` is not accepted",
            component.as_str(),
            component.as_str()
        )
    }
}

fn require_worker_flag(component: ServeComponent, flag: &str) -> Result<(), String> {
    if component == ServeComponent::Worker {
        Ok(())
    } else {
        Err(format!(
            "`{flag}` cannot be used with `temper serve {}`; use it with `temper serve worker`",
            component.as_str()
        ))
    }
}

fn next_non_empty_serve_value(
    component: ServeComponent,
    iter: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, String> {
    let value = next_serve_value(component, iter, flag)?;
    if value.trim().is_empty() {
        return Err(format!(
            "`{flag}` requires a non-empty value for `temper serve {}`",
            component.as_str()
        ));
    }
    Ok(value)
}

fn next_serve_value(
    component: ServeComponent,
    iter: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, String> {
    let Some(value) = iter.next() else {
        return Err(format!(
            "`{flag}` requires a value for `temper serve {}`",
            component.as_str()
        ));
    };
    if value.starts_with("--") {
        return Err(format!(
            "`{flag}` requires a value for `temper serve {}`",
            component.as_str()
        ));
    }
    Ok(value)
}

fn parse_serve_capacity(raw: &str) -> Result<u32, String> {
    let capacity = raw.parse::<u32>().map_err(|_| {
        format!("invalid --capacity `{raw}` (expected a positive integer greater than zero)")
    })?;
    if capacity == 0 {
        return Err("--capacity must be greater than zero".to_string());
    }
    Ok(capacity)
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

fn apply_runtime_overrides(
    resolved: &mut Resolved,
    service: Option<Service>,
    runtime: &RuntimeOverrides,
) -> Result<(), String> {
    match service {
        None => apply_standalone_runtime_overrides(resolved, runtime),
        Some(Service::Engine) => apply_engine_runtime_overrides(resolved, runtime),
        Some(Service::Worker) => apply_worker_runtime_overrides(resolved, runtime),
    }
}

fn apply_standalone_runtime_overrides(
    resolved: &mut Resolved,
    runtime: &RuntimeOverrides,
) -> Result<(), String> {
    reject_worker_only_runtime_overrides(runtime, "standalone")?;
    if let Some(process_id) = runtime.process_id.as_deref() {
        let process_id = non_empty_runtime_override("--id", process_id)?;
        resolved.engine.daemon_id = process_id.to_string();
        resolved.worker.worker_id = process_id.to_string();
    }
    Ok(())
}

fn apply_engine_runtime_overrides(
    resolved: &mut Resolved,
    runtime: &RuntimeOverrides,
) -> Result<(), String> {
    reject_worker_only_runtime_overrides(runtime, "engine")?;
    if let Some(process_id) = runtime.process_id.as_deref() {
        resolved.engine.daemon_id = non_empty_runtime_override("--id", process_id)?.to_string();
    }
    Ok(())
}

fn apply_worker_runtime_overrides(
    resolved: &mut Resolved,
    runtime: &RuntimeOverrides,
) -> Result<(), String> {
    if let Some(process_id) = runtime.process_id.as_deref() {
        resolved.worker.worker_id = non_empty_runtime_override("--id", process_id)?.to_string();
    }
    if let Some(engine_url) = runtime.worker_engine_url.as_deref() {
        resolved.worker.daemon_url =
            non_empty_runtime_override("--engine-url", engine_url)?.to_string();
    }
    if let Some(pool_name) = runtime.worker_pool.as_deref() {
        let pool_name = non_empty_runtime_override("--pool", pool_name)?;
        let pool = resolved
            .worker
            .pools
            .iter()
            .find(|pool| pool.name == pool_name)
            .ok_or_else(|| format!("unknown worker pool `{pool_name}`"))?;
        let capabilities = capabilities_from_pool(pool)?;
        if let Some(capacity) = pool.max_concurrent_jobs {
            resolved.worker.max_concurrent_jobs = capacity;
        }
        resolved.worker.capabilities = capabilities;
    }
    if let Some(capacity) = runtime.worker_capacity {
        if capacity == 0 {
            return Err("--capacity must be greater than zero".to_string());
        }
        resolved.worker.max_concurrent_jobs = capacity;
    }
    Ok(())
}

fn reject_worker_only_runtime_overrides(
    runtime: &RuntimeOverrides,
    component: &str,
) -> Result<(), String> {
    if let Some(pool) = &runtime.worker_pool {
        return Err(format!(
            "`--pool` cannot be used with `temper serve {component}` (got `{pool}`); use `temper serve worker`"
        ));
    }
    if runtime.worker_capacity.is_some() {
        return Err(format!(
            "`--capacity` cannot be used with `temper serve {component}`; use `temper serve worker`"
        ));
    }
    if let Some(url) = &runtime.worker_engine_url {
        return Err(format!(
            "`--engine-url` cannot be used with `temper serve {component}` (got `{url}`); use `temper serve worker`"
        ));
    }
    Ok(())
}

fn capabilities_from_pool(pool: &WorkerPoolSettings) -> Result<Vec<Capability>, String> {
    if pool.roles.is_empty() {
        return Err(format!(
            "worker pool `{}` does not declare any roles, so it cannot produce runtime capabilities",
            pool.name
        ));
    }
    if pool.repos.is_empty() {
        return Err(format!(
            "worker pool `{}` does not declare any repos, so it cannot produce runtime capabilities",
            pool.name
        ));
    }

    let mut capabilities = Vec::with_capacity(pool.roles.len() * pool.repos.len());
    for repo in &pool.repos {
        for role in &pool.roles {
            capabilities.push(Capability {
                repo: repo.display(),
                role: role.clone(),
            });
        }
    }
    Ok(capabilities)
}

fn non_empty_runtime_override<'a>(flag: &str, value: &'a str) -> Result<&'a str, String> {
    if value.trim().is_empty() {
        Err(format!("{flag} requires a non-empty value"))
    } else {
        Ok(value)
    }
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
