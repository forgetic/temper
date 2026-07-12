// SPDX-License-Identifier: MPL-2.0

//! Worker runtime wiring and agent command-prefix helpers.

use std::sync::Arc;

use temper_config::Resolved;
use temper_worker::{CodingExecutor, CodingExecutorConfig, OutOfProcessRunner, run_worker};

use crate::adapt;

/// Runs the worker on the skein runtime until the process is signalled.
///
/// `agent_program` is the command prefix used to launch the coding agent — e.g.
/// [`sibling_program`]`("temper-agent")` for the slim binary, or
/// [`self_subcommand`]`("agent")` for the unified binary.
pub fn run(resolved: &Resolved, agent_program: Vec<String>) -> Result<(), String> {
    let resolved = resolved.clone();
    temper_worker_io::block_on_with(move |cx, handle| async move {
        run_async(cx, handle, &resolved, &agent_program).await
    })
}

async fn run_async(
    cx: skein::cx::Cx,
    handle: skein::runtime::RuntimeHandle,
    resolved: &Resolved,
    agent_program: &[String],
) -> Result<(), String> {
    let worker_config = adapt::worker_config(resolved)?;
    let git_base_url = adapt::git_base_url(resolved)?;
    let workspace_root = resolved.worker.workspace_root.clone();

    let invocation = adapt::agent_invocation(resolved, agent_program)?;
    let forge_context = temper_worker::forge_context_host(
        Arc::new(temper_worker::HttpTransport::new(&worker_config.daemon_url)),
        cx,
        worker_config.worker_id.clone(),
        worker_config.worker_auth.clone(),
    );
    let runner = Arc::new(
        OutOfProcessRunner::new(invocation.command)
            .with_env(invocation.env)
            .with_tool_config(invocation.tool_config)
            .with_forge_context_host(forge_context),
    );

    // The coding executor's identities come from the worker config — the worker
    // subsystem's single source of truth for role identities (issue #199).
    let executor_config = CodingExecutorConfig {
        workspace_root,
        git_base_url,
        role_identities: worker_config.role_identities.clone(),
    };

    let executor = Arc::new(CodingExecutor::new(executor_config, runner));

    run_worker(handle, worker_config, executor)
        .await
        .map_err(|error| error.to_string())
}

/// The agent invocation prefix `[<current-exe>, <subcommand>]` — used by the
/// unified binary so the worker spawns the same binary's `agent` subcommand.
pub fn self_subcommand(subcommand: &str) -> Vec<String> {
    let program = std::env::current_exe()
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "temper".to_string());
    vec![program, subcommand.to_string()]
}

/// The agent invocation prefix for a sibling binary `name`, resolved next to the
/// current executable when present (falling back to bare `name` on `PATH`). Used
/// by the slim `temper-worker` binary to spawn `temper-agent`.
pub fn sibling_program(name: &str) -> Vec<String> {
    let program = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.exists())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    vec![program]
}
