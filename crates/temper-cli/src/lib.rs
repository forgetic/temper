// SPDX-License-Identifier: MPL-2.0

//! The unified `temper` command line — a thin dispatcher.
//!
//! [`run`] dispatches `argv[1]` to the headline subcommands — `init`, `config`,
//! `serve`, `daemon`, `agent` — and to the hidden operator/responder tools. Each
//! subcommand lives in its own crate (`temper-cli-init`, `temper-cli-config`,
//! `temper-cli-daemon`, `temper-agent-session`); this crate owns only the
//! dispatch table and the operator/responder wrappers, so the heavy
//! engine/worker/agent wiring (under `temper-cli-daemon`) is pulled in only when
//! the daemon path is built.

mod operators;
mod responders;

use std::process::ExitCode;

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
  init    Interactively configure and provision a deployment
  serve   Run a long-lived Temper process (standalone supported)
  config  Guided or programmatic configuration
  daemon  Run a full standalone daemon or one of its components (engine, worker)
  agent   Run an agent session (usually invoked by the daemon)

Options:
  --config  <DIR|FILE>  Path to configuration file or bundle directory
  --secrets <DIR|FILE>  Explicit secret source directory or credentials.toml
  -h, --help            Print help
  -V, --version         Print version

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
    let rest = apply_global_args(&command, parsed.rest, parsed.globals);
    dispatch(&command, rest, &env, &paths)
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ParsedTopLevelArgs {
    command: Option<String>,
    rest: Vec<String>,
    globals: Vec<String>,
}

/// Parses leading global file-location flags before the subcommand. Subcommand
/// parsers still accept the same flags in their local position; this pass exists
/// so the long-term UX form (`temper --config … --secrets … serve standalone`)
/// reaches the same code paths.
fn parse_top_level_args(args: Vec<String>) -> Result<ParsedTopLevelArgs, String> {
    let mut iter = args.into_iter();
    let mut globals = Vec::new();
    while let Some(arg) = iter.next() {
        if matches!(arg.as_str(), "--config" | "--secrets") {
            let value = next_global_value(&mut iter, &arg)?;
            globals.push(arg);
            globals.push(value);
        } else {
            return Ok(ParsedTopLevelArgs {
                command: Some(arg),
                rest: iter.collect(),
                globals,
            });
        }
    }
    Ok(ParsedTopLevelArgs {
        command: None,
        rest: Vec::new(),
        globals,
    })
}

fn next_global_value(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    iter.next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn apply_global_args(command: &str, rest: Vec<String>, globals: Vec<String>) -> Vec<String> {
    if globals.is_empty() {
        return rest;
    }
    match command {
        "init" | "daemon" => prepend_globals(globals, rest),
        // These commands reserve argv[0] after the command for a nested action
        // (`config validate`) or component (`serve standalone`), so global flags
        // must be inserted after that token rather than before it.
        "config" | "serve" => insert_globals_after_first(rest, globals),
        _ => rest,
    }
}

fn prepend_globals(mut globals: Vec<String>, rest: Vec<String>) -> Vec<String> {
    globals.extend(rest);
    globals
}

fn insert_globals_after_first(rest: Vec<String>, globals: Vec<String>) -> Vec<String> {
    let mut iter = rest.into_iter();
    let Some(first) = iter.next() else {
        return Vec::new();
    };
    let mut out = vec![first];
    out.extend(globals);
    out.extend(iter);
    out
}

/// Dispatch a command + its remaining args over the injected `env` / `paths`
/// snapshot. Separated from [`run`] for testing.
pub fn dispatch(
    command: &str,
    args: Vec<String>,
    env: &temper_cli_common::EnvMap,
    paths: &temper_cli_common::PathResolver,
) -> ExitCode {
    match command {
        "init" => temper_cli_init::main(args, env, paths),
        "serve" => temper_cli_daemon::serve_main(args, env, paths),
        "config" => temper_cli_config::main(temper_cli_config::ConfigInputs { args, env, paths }),
        "daemon" => temper_cli_daemon::main(args, env, paths),
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
    use super::{USAGE, apply_global_args, parse_top_level_args};

    #[test]
    fn top_level_usage_lists_serve_command() {
        assert!(USAGE.contains("serve"));
        assert!(USAGE.contains("standalone supported"));
        assert!(USAGE.contains("--secrets"));
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
            parsed.globals,
            vec![
                "--config".to_string(),
                "deploy/config.toml".to_string(),
                "--secrets".to_string(),
                "deploy/credentials.toml".to_string(),
            ]
        );
    }

    #[test]
    fn global_options_are_inserted_after_serve_component() {
        let args = apply_global_args(
            "serve",
            vec!["standalone".to_string(), "--help".to_string()],
            vec![
                "--config".to_string(),
                "deploy/config.toml".to_string(),
                "--secrets".to_string(),
                "deploy/credentials.toml".to_string(),
            ],
        );

        assert_eq!(
            args,
            vec![
                "standalone".to_string(),
                "--config".to_string(),
                "deploy/config.toml".to_string(),
                "--secrets".to_string(),
                "deploy/credentials.toml".to_string(),
                "--help".to_string(),
            ]
        );
    }
}
