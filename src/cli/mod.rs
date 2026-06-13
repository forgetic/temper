// SPDX-License-Identifier: MPL-2.0

//! Subcommand entrypoints for the `temper` product.
//!
//! Every subcommand is a `pub fn main(args) -> ExitCode` — the single source of
//! truth shared by the unified `temper` dispatcher and the split binaries. The
//! modules are feature-gated so a `--no-default-features --features worker`
//! build compiles only the worker path (and never links the agent's LLM/HTTP
//! stack).

use std::process::ExitCode;

#[cfg(feature = "daemon")]
pub mod daemon;
#[cfg(feature = "daemon")]
pub mod interaction;
#[cfg(feature = "daemon")]
pub mod provision_forgejo;
#[cfg(feature = "daemon")]
pub mod trigger_forgejo;
#[cfg(feature = "daemon")]
pub mod validate_reference_delivery;

#[cfg(feature = "worker")]
pub mod worker;

#[cfg(feature = "agent")]
pub mod agent;
#[cfg(feature = "agent")]
pub mod product_manager_responder;
#[cfg(feature = "agent")]
pub mod workflow_role_decision;

/// Exit code for a usage error (no/unknown subcommand, or a subcommand whose
/// feature was not compiled in). Matches `EX_USAGE` from `sysexits.h`.
pub const EX_USAGE: u8 = 64;

/// Top-level usage banner for the unified binary.
pub const USAGE: &str = "\
temper — unified daemon/worker/agent for an automated software factory

usage: temper <subcommand> [args...]

subcommands:
  run                          all-in-one: daemon + worker + agent in one process
  daemon                       orchestrator: route work, apply results, land PRs
  worker                       orchestration worker: long-poll the daemon, run jobs
  agent                        out-of-process coding agent (worker spawns this)
  provision-forgejo            provision a Forgejo instance for a workflow
  trigger-forgejo              trigger a Forgejo role feed
  validate-reference-delivery  validate a reference-delivery run
  interaction                  interactive responder (REPL or serve)

Run `temper <subcommand> --help` for subcommand usage.";

/// Reports a subcommand that exists but was not compiled into this build.
///
/// Only referenced by the `#[cfg(not(feature = ...))]` dispatch arms, so in any
/// build where all of daemon/worker/agent are present it is dead — that is
/// expected (the slim single-role builds are the ones that use it), not a smell.
#[allow(dead_code)]
fn not_compiled_in(subcommand: &str, feature: &str) -> ExitCode {
    eprintln!(
        "temper: subcommand `{subcommand}` is not available in this build \
         (compiled without the `{feature}` feature); rebuild with \
         `--features {feature}`"
    );
    ExitCode::from(EX_USAGE)
}

/// Dispatch `argv[1..]` to the matching subcommand.
///
/// `subcommand` is `argv[1]`; `args` is everything after it (what each
/// subcommand's own `parse` expects). A subcommand whose feature was not
/// compiled in returns a clear "rebuild with --features X" error, distinct from
/// an unknown subcommand.
pub fn dispatch<I>(subcommand: &str, args: I) -> ExitCode
where
    I: Iterator<Item = String>,
{
    match subcommand {
        "run" => run_all_in_one(args),

        #[cfg(feature = "daemon")]
        "daemon" => daemon::main(args),
        #[cfg(not(feature = "daemon"))]
        "daemon" => not_compiled_in("daemon", "daemon"),

        #[cfg(feature = "worker")]
        "worker" => worker::main(args),
        #[cfg(not(feature = "worker"))]
        "worker" => not_compiled_in("worker", "worker"),

        #[cfg(feature = "agent")]
        "agent" => agent::main(args),
        #[cfg(not(feature = "agent"))]
        "agent" => not_compiled_in("agent", "agent"),

        #[cfg(feature = "daemon")]
        "provision-forgejo" => provision_forgejo::main(args),
        #[cfg(not(feature = "daemon"))]
        "provision-forgejo" => not_compiled_in("provision-forgejo", "daemon"),

        #[cfg(feature = "daemon")]
        "trigger-forgejo" => trigger_forgejo::main(args),
        #[cfg(not(feature = "daemon"))]
        "trigger-forgejo" => not_compiled_in("trigger-forgejo", "daemon"),

        #[cfg(feature = "daemon")]
        "validate-reference-delivery" => validate_reference_delivery::main(args),
        #[cfg(not(feature = "daemon"))]
        "validate-reference-delivery" => not_compiled_in("validate-reference-delivery", "daemon"),

        #[cfg(feature = "daemon")]
        "interaction" => interaction::main(args),
        #[cfg(not(feature = "daemon"))]
        "interaction" => not_compiled_in("interaction", "daemon"),

        #[cfg(feature = "agent")]
        "workflow-role-decision" => workflow_role_decision::main(args),
        #[cfg(not(feature = "agent"))]
        "workflow-role-decision" => not_compiled_in("workflow-role-decision", "agent"),

        #[cfg(feature = "agent")]
        "product-manager-responder" => product_manager_responder::main(args),
        #[cfg(not(feature = "agent"))]
        "product-manager-responder" => not_compiled_in("product-manager-responder", "agent"),

        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("temper: unknown subcommand `{other}`\n\n{USAGE}");
            ExitCode::from(EX_USAGE)
        }
    }
}

/// The all-in-one solo-dev mode (`temper run`). Wired in Phase B; for now it
/// reports that it is not yet available rather than silently doing nothing.
fn run_all_in_one<I>(_args: I) -> ExitCode
where
    I: Iterator<Item = String>,
{
    eprintln!(
        "temper run: the single-process all-in-one mode is not wired up yet \
         (Phase B). Use `temper daemon`, `temper worker`, and `temper agent` \
         as separate processes for now."
    );
    ExitCode::from(EX_USAGE)
}
