// SPDX-License-Identifier: MPL-2.0

//! The unified `temper` command line.
//!
//! [`run`] dispatches `argv[1]` to the three headline subcommands — `config`,
//! `daemon`, `agent` — and to the hidden operator tools. The daemon subcommand
//! runs the whole stack standalone or one service at a time (`--service
//! engine|worker`); the slim per-service binaries (`temper-engine`,
//! `temper-worker`, `temper-agent`) run the same code with no extra plumbing.

mod config_cmd;
mod daemon;
mod operators;
mod provider;
mod responders;
mod standalone;

use std::process::ExitCode;

use temper_config::EX_USAGE;

// Exposed for the root package's in-process-transport integration test, which
// proves the standalone worker→daemon carrier in isolation.
pub use standalone::{InProcessAgentRunner, InProcessTransport};

/// Top-level usage, shown for `temper`, `temper --help`, and unknown commands.
pub const USAGE: &str = "\
temper: Issue-tracker-native execution engine for agentic workflows

Run agentic workflows on top of your Forge.

Temper is a workflow runtime that executes agentic workflows using your Forge as
the source of truth.

Usage: temper [COMMAND]

Commands:
  config  Guided or programmatic configuration
  daemon  Run a full standalone daemon or one of its components (engine, worker)
  agent   Run an agent session (usually invoked by the daemon)

Options:
  -h, --help     Print help
  -V, --version  Print version

Run `temper <command> --help` for subcommand usage.";

/// The unified binary's entry point: parse `argv[1]` and dispatch.
pub fn run() -> ExitCode {
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
        "config" => config_cmd::main(args),
        "daemon" => daemon::main(args),
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
