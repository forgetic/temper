// SPDX-License-Identifier: MPL-2.0

//! The unified `temper` command line — a thin dispatcher.
//!
//! [`run`] dispatches `argv[1]` to the headline subcommands — `init`, `apply`,
//! `config`, `serve`, `daemon` — plus the internal agent entry point and the
//! hidden operator/responder tools. Each
//! subcommand lives in its own crate (`temper-cli-init`, `temper-cli-config`,
//! `temper-cli-daemon`, `temper-agent-session`); this crate owns only the
//! dispatch table and the operator/responder wrappers, so the heavy
//! engine/worker/agent wiring (under `temper-cli-daemon`) is pulled in only when
//! the daemon path is built.

mod operators;
mod responders;

use std::path::PathBuf;
use std::process::ExitCode;

use temper_cli_common::{GlobalOptions, OutputFormat};
use temper_config::EX_USAGE;

// Re-exported so `src/bin/*` and tests construct the CLI's injected environment
// snapshot with `temper_cli::CliEnv`.
pub use temper_cli_common::CliEnv;

// Re-exported from `temper-cli-daemon` so the root package's in-process-transport
// integration test and `src/bin/temper.rs` keep the same `temper_cli::*` paths.
pub use temper_cli_daemon::{InProcessAgentRunner, InProcessTransport};

/// Top-level usage, shown for `temper`, `temper --help`, and unknown commands.
pub const USAGE: &str = "\
temper: Issue-tracker-native execution engine for agentic workflows

Run agentic workflows on top of your Forge.

Temper is a workflow runtime that executes agentic workflows using your Forge as
the source of truth.

Usage: temper [OPTIONS] [COMMAND]

Commands:
  init    Interactively configure a deployment bundle
  apply   Provision a deployment bundle on the forge
  serve   Run a long-lived Temper process (standalone supported)
  config  Guided or programmatic configuration
  daemon  Run a full standalone daemon or one of its components (engine, worker)

Options:
  -c, --config <DIR|FILE>      Path to configuration file or bundle directory
      --secrets <DIR|FILE>     Explicit secret source directory or credentials.toml
      --format <human|json>    Output format for commands that support it
  -h, --help                  Print help
  -V, --version           Print version

Run `temper <command> --help` for subcommand usage.";

/// The unified binary's entry point: dispatch `argv[1]` off an injected
/// environment snapshot.
///
/// The snapshot ([`CliEnv`]) is captured once at the composition root
/// (`src/bin/temper.rs`'s `boot()`); no library code reads `std::env`. The
/// per-subcommand crates take the snapshot's `env` / `paths` so a load is
/// hermetic with respect to whatever was captured.
pub fn run(cli: CliEnv) -> ExitCode {
    // Install the global tracing subscriber before any work (or log output)
    // happens, so early CLI errors and startup spans/events are captured. This
    // is the single install point for the unified binary — it covers every
    // subcommand, so individual subcommands must not call it again. Idempotent,
    // so chaining is safe regardless.
    temper_log::init_logging();

    let CliEnv {
        args,
        env,
        paths,
        cwd: _cwd,
    } = cli;
    let parsed = match parse_top_level_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("temper: {error}\n\n{USAGE}");
            return ExitCode::from(EX_USAGE);
        }
    };
    let Some(command) = parsed.command else {
        println!("{USAGE}");
        return ExitCode::from(EX_USAGE);
    };
    dispatch(&command, parsed.rest, &env, &paths, parsed.globals)
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ParsedTopLevelArgs {
    command: Option<String>,
    rest: Vec<String>,
    globals: GlobalOptions,
}

/// Parses leading global options before the subcommand. The long-term UX
/// accepts these options only in this leading position, for example
/// `temper --config … --secrets … --format json serve standalone`.
fn parse_top_level_args(args: Vec<String>) -> Result<ParsedTopLevelArgs, String> {
    let mut iter = args.into_iter();
    let mut globals = GlobalOptions::default();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-c" | "--config" => {
                globals.load.config = Some(PathBuf::from(next_global_value(&mut iter, &arg)?));
            }
            "--secrets" => {
                globals.load.credentials = Some(PathBuf::from(next_global_value(&mut iter, &arg)?));
            }
            "--format" => {
                let value = next_global_value(&mut iter, &arg)?;
                globals.format = OutputFormat::parse(&value).ok_or_else(|| {
                    format!("invalid --format `{value}` (expected `human` or `json`)")
                })?;
            }
            _ => {
                return Ok(ParsedTopLevelArgs {
                    command: Some(arg),
                    rest: iter.collect(),
                    globals,
                });
            }
        }
    }
    Ok(ParsedTopLevelArgs {
        command: None,
        rest: Vec::new(),
        globals,
    })
}

fn next_global_value(
    iter: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, String> {
    iter.next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| format!("{flag} requires a value"))
}

/// Dispatch a command + its remaining args over the injected `env` / `paths`
/// snapshot. Separated from [`run`] for testing.
pub fn dispatch(
    command: &str,
    args: Vec<String>,
    env: &temper_cli_common::EnvMap,
    paths: &temper_cli_common::PathResolver,
    globals: GlobalOptions,
) -> ExitCode {
    match command {
        "init" => temper_cli_init::main_with_options(args, env, paths, globals.load),
        "apply" => temper_cli_init::apply_main_with_options(args, env, paths, globals.load),
        "serve" => temper_cli_daemon::serve_main_with_options(args, env, paths, globals.load),
        "config" => temper_cli_config::main(temper_cli_config::ConfigInputs {
            args,
            options: globals.load,
            format: globals.format,
            env,
            paths,
        }),
        "daemon" => temper_cli_daemon::main_with_options(args, env, paths, globals.load),
        // The agent is its own process entry point and reads the worker-injected
        // env through its sanctioned `entry` module (issue #201); it needs no
        // snapshot from here.
        "agent" => temper_agent_session::main(args.into_iter()),

        // Hidden operator/responder tools — not in the headline help, but kept
        // dispatchable for tests, provisioning, and the agent's responder
        // subprocesses. These are process entry points that read their own
        // (allowlisted) env, so they take only their args.
        "provision-forgejo" => operators::provision_forgejo(args),
        "trigger-forgejo" => operators::trigger_forgejo(args),
        "validate-reference-delivery" => operators::validate_reference_delivery(args),
        "interaction" => operators::interaction(args),
        "product-manager-responder" => responders::product_manager(args),

        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        "-V" | "--version" => {
            println!("temper {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("temper: unknown command `{other}`\n\n{USAGE}");
            ExitCode::from(EX_USAGE)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::ExitCode;

    use temper_cli_common::{EnvMap, GlobalOptions, OutputFormat, PathResolver};

    use super::{USAGE, dispatch, parse_top_level_args};

    #[test]
    fn top_level_usage_lists_headline_commands_but_hides_internal_agent() {
        assert!(USAGE.contains("\n  apply "));
        assert!(USAGE.contains("\n  serve "));
        assert!(USAGE.contains("standalone supported"));
        assert!(USAGE.contains("--secrets"));
        assert!(!USAGE.contains("\n  agent "), "{USAGE}");
    }

    #[test]
    fn internal_agent_help_remains_dispatchable() {
        let env = EnvMap::new();
        let paths = PathResolver::default();

        assert_eq!(
            dispatch(
                "agent",
                vec!["--help".to_string()],
                &env,
                &paths,
                GlobalOptions::default()
            ),
            ExitCode::SUCCESS
        );
    }

    #[test]
    fn leading_config_and_secrets_are_global_options() {
        let parsed = parse_top_level_args(vec![
            "--config".to_string(),
            "deploy/config.toml".to_string(),
            "--secrets".to_string(),
            "deploy/credentials.toml".to_string(),
            "serve".to_string(),
            "standalone".to_string(),
        ])
        .expect("global args parse");

        assert_eq!(parsed.command.as_deref(), Some("serve"));
        assert_eq!(parsed.rest, vec!["standalone".to_string()]);
        assert_eq!(
            parsed.globals.load.config,
            Some(PathBuf::from("deploy/config.toml"))
        );
        assert_eq!(
            parsed.globals.load.credentials,
            Some(PathBuf::from("deploy/credentials.toml"))
        );
        assert_eq!(parsed.globals.format, OutputFormat::Human);
    }

    #[test]
    fn short_config_is_a_global_option() {
        let parsed = parse_top_level_args(vec![
            "-c".to_string(),
            "deploy".to_string(),
            "init".to_string(),
            "--yes".to_string(),
        ])
        .expect("global args parse");

        assert_eq!(parsed.command.as_deref(), Some("init"));
        assert_eq!(parsed.rest, vec!["--yes".to_string()]);
        assert_eq!(parsed.globals.load.config, Some(PathBuf::from("deploy")));
        assert_eq!(parsed.globals.load.credentials, None);
    }

    #[test]
    fn config_and_secrets_after_command_are_not_global_options() {
        let parsed = parse_top_level_args(vec![
            "serve".to_string(),
            "standalone".to_string(),
            "--config".to_string(),
            "deploy/config.toml".to_string(),
        ])
        .expect("top-level parse succeeds");

        assert_eq!(parsed.command.as_deref(), Some("serve"));
        assert_eq!(
            parsed.rest,
            vec![
                "standalone".to_string(),
                "--config".to_string(),
                "deploy/config.toml".to_string(),
            ]
        );
        assert_eq!(parsed.globals.load.config, None);
        assert_eq!(parsed.globals.load.credentials, None);
        assert_eq!(parsed.globals.format, OutputFormat::Human);
    }

    #[test]
    fn leading_format_is_a_global_option() {
        let parsed = parse_top_level_args(vec![
            "--format".to_string(),
            "json".to_string(),
            "config".to_string(),
            "paths".to_string(),
        ])
        .expect("global args parse");

        assert_eq!(parsed.command.as_deref(), Some("config"));
        assert_eq!(parsed.rest, vec!["paths".to_string()]);
        assert_eq!(parsed.globals.load.config, None);
        assert_eq!(parsed.globals.load.credentials, None);
        assert_eq!(parsed.globals.format, OutputFormat::Json);
    }

    #[test]
    fn invalid_global_format_errors() {
        let err = parse_top_level_args(vec![
            "--format".to_string(),
            "yaml".to_string(),
            "config".to_string(),
        ])
        .expect_err("invalid format errors");

        assert!(err.contains("invalid --format"), "{err}");
        assert!(err.contains("human"), "{err}");
        assert!(err.contains("json"), "{err}");
    }

    #[test]
    fn format_after_command_is_not_a_global_option() {
        let parsed = parse_top_level_args(vec![
            "config".to_string(),
            "--format".to_string(),
            "json".to_string(),
            "paths".to_string(),
        ])
        .expect("top-level parse succeeds");

        assert_eq!(parsed.command.as_deref(), Some("config"));
        assert_eq!(
            parsed.rest,
            vec![
                "--format".to_string(),
                "json".to_string(),
                "paths".to_string(),
            ]
        );
        assert_eq!(parsed.globals.format, OutputFormat::Human);
    }
}
