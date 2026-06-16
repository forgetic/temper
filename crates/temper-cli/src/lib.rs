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

/// The unified binary's entry point: parse `argv[1]` and dispatch.
pub fn run() -> ExitCode {
    // Install the global tracing subscriber before any work (or log output)
    // happens, so early CLI errors and startup spans/events are captured. This
    // is the single install point for the unified binary — it covers every
    // subcommand, so individual subcommands must not call it again. Idempotent,
    // so chaining is safe regardless.
    temper_log::init_logging();

    let mut args = std::env::args();
    let _program = args.next();
    let Some(command) = args.next() else {
        println!("{USAGE}");
        return ExitCode::from(EX_USAGE);
    };
    dispatch(&command, args)
}

/// Dispatch a command + its remaining args. Separated from [`run`] for testing.
pub fn dispatch(command: &str, args: std::env::Args) -> ExitCode {
    match command {
        "init" => temper_cli_init::main(args),
        "config" => temper_cli_config::main(args),
        "daemon" => temper_cli_daemon::main(args),
        "agent" => temper_agent_session::main(args),

        // Hidden operator/responder tools — not in the headline help, but kept
        // dispatchable for tests, provisioning, and the agent's responder
        // subprocesses.
        "provision-forgejo" => operators::provision_forgejo(args),
        "trigger-forgejo" => operators::trigger_forgejo(args),
        "validate-reference-delivery" => operators::validate_reference_delivery(args),
        "interaction" => operators::interaction(args),
        "product-manager-responder" => responders::product_manager(args),
        "workflow-role-decision" => responders::workflow_role_decision(args),

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
