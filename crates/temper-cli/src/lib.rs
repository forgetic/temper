// SPDX-License-Identifier: MPL-2.0

//! The unified `temper` command line — a thin dispatcher.
//!
//! [`run`] dispatches `argv[1]` to the headline subcommands — `init`, `config`,
//! `daemon`, `agent` — and to the hidden operator/responder tools. Each
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

Usage: temper [COMMAND]

Commands:
  init    Interactively configure and provision a deployment
  config  Guided or programmatic configuration
  daemon  Run a full standalone daemon or one of its components (engine, worker)
  agent   Run an agent session (usually invoked by the daemon)

Options:
  -h, --help     Print help
  -V, --version  Print version

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
    let mut iter = args.into_iter();
    let Some(command) = iter.next() else {
        println!("{USAGE}");
        return ExitCode::from(EX_USAGE);
    };
    let rest: Vec<String> = iter.collect();
    dispatch(&command, rest, &env, &paths)
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
