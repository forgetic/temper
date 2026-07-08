// SPDX-License-Identifier: MPL-2.0

//! Parser and help text for `temper serve` target-era compatibility flags.

use crate::{RuntimeOverrides, Service};

pub const SERVE_USAGE: &str = "\
Run a Temper process.

Usage: temper [GLOBAL OPTIONS] serve <COMPONENT> [OPTIONS]

Components:
  standalone  Run all Temper planes in one local process
  engine      Run the engine service and Forgejo webhook endpoint
  worker      Run one worker process, optionally scoped to a worker pool
  trigger     No separate process; engine handles webhook hints

Options:
  -h, --help  Print help

Component options:
  standalone  --id <ID>
  engine      --id <ID>
  worker      --pool <NAME> --id <ID> --capacity <N> --engine-url <URL>

Place `--config` / `--secrets` before `serve`, for example:
  temper --config ./deploy --secrets ./deploy/credentials.toml serve engine

Legacy `temper daemon` forms remain dispatchable for existing automation, but
new deployments should use `temper serve <component>`.

Run `temper serve <component> --help` for component-specific usage.";

pub const SERVE_STANDALONE_USAGE: &str = "\
Run Temper in standalone mode.

The engine, worker, webhook intake, poll backstops, and agent execution run in
one local process. Use this for demos and small single-host deployments.

Usage: temper [GLOBAL OPTIONS] serve standalone [--id <ID>]

Options:
      --id <ID>  Stable process identity for the all-in-one runtime
  -h, --help    Print help";

pub const SERVE_ENGINE_USAGE: &str = "\
Run the Temper engine service.

The engine owns queue scheduling, the worker protocol, Forgejo webhook intake at
`/forgejo/webhook`, and poll/mechanical backstops. Put deployment file flags
before `serve`:
  temper --config ./deploy --secrets ./deploy/credentials.toml serve engine

Usage: temper [GLOBAL OPTIONS] serve engine [--id <ID>]

Options:
      --id <ID>  Override the engine process identity
  -h, --help    Print help";

pub const SERVE_WORKER_USAGE: &str = "\
Run the Temper worker service.

The worker long-polls the engine for jobs and may be scoped to one configured
`[[worker.pools]]` capability class. Put deployment file flags before `serve`:
  temper --config ./deploy --secrets ./deploy/credentials.toml serve worker

Usage: temper [GLOBAL OPTIONS] serve worker [OPTIONS]

Options:
      --pool <NAME>       Select a resolved [[worker.pools]] capability class
      --id <ID>           Override the worker registration/logging identity
      --capacity <N>      Override max concurrent jobs for this worker (N > 0)
      --engine-url <URL>  Override the engine URL for this worker
  -h, --help             Print help";

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ServeInvocation {
    Help,
    Version,
    StandaloneHelp,
    ServiceHelp(Service),
    Standalone(RuntimeOverrides),
    Service(Service, RuntimeOverrides),
}

pub(crate) fn serve_service_usage(service: Service) -> &'static str {
    match service {
        Service::Engine => SERVE_ENGINE_USAGE,
        Service::Worker => SERVE_WORKER_USAGE,
    }
}

pub(crate) fn parse_serve_invocation(args: Vec<String>) -> Result<ServeInvocation, String> {
    let mut iter = args.into_iter();
    let Some(component) = iter.next() else {
        return Err("missing component (expected `standalone`, `engine`, or `worker`)".to_string());
    };
    match component.as_str() {
        "-h" | "--help" | "help" => Ok(ServeInvocation::Help),
        "-V" | "--version" => Ok(ServeInvocation::Version),
        "-c" | "--config" | "--secrets" => Err(format!(
            "`{component}` is a global option; place it before `serve`"
        )),
        "standalone" => parse_serve_standalone(iter.collect()),
        "engine" => parse_serve_service(Service::Engine, iter.collect()),
        "worker" => parse_serve_service(Service::Worker, iter.collect()),
        "trigger" => Err(
            "`temper serve trigger` has no separate process; run `temper serve engine` \
             with a configured webhook secret. Webhooks are wake hints and polling \
             remains the correctness backstop."
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
                return Err(format!(
                    "`{arg}` is a global option; place it before `serve`"
                ));
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
