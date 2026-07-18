// SPDX-License-Identifier: MPL-2.0

//! Worker runtime wiring and agent command-prefix helpers.

use std::sync::Arc;

use temper_config::Resolved;
use temper_worker::{
    CodingExecutor, CodingExecutorConfig, HttpTransport, OutOfProcessRunner, TraceCollector,
    start_worker_with_transport_and_trace_collector,
};

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
    if resolved.observability.agent_traces.capture_requested()
        && worker_config.agent_traces.spool_root.is_none()
    {
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            "agent tracing disabled: no durable paths.state_dir is available for the worker spool"
        );
    }
    let git_base_url = adapt::git_base_url(resolved)?;
    let workspace_root = resolved.worker.workspace_root.clone();

    let invocation = adapt::agent_invocation(resolved, agent_program)?;
    debug_assert_eq!(
        invocation.runtime_limits.is_some(),
        matches!(
            invocation.supervision,
            adapt::AgentSupervisionKind::FirstParty
        )
    );
    let transport = Arc::new(HttpTransport::new(&worker_config.daemon_url));
    let trace_collector = TraceCollector::new(worker_config.agent_traces.clone());
    let forge_context = temper_worker::forge_context_host(
        Arc::clone(&transport),
        cx,
        worker_config.worker_id.clone(),
        worker_config.worker_auth.clone(),
    );
    let runner = Arc::new(
        OutOfProcessRunner::new(invocation.command)
            .with_env(invocation.env)
            .with_tool_config(invocation.tool_config)
            .with_runtime_limits(invocation.runtime_limits)
            .with_liveness_limits(worker_config.liveness_limits)
            .with_trace_policy(invocation.trace_policy)
            .with_shared_trace_collector(trace_collector.clone())
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
    let mut sigint = skein::signal::sigint()
        .map_err(|error| format!("failed to register SIGINT handler: {error}"))?;
    let mut sigterm = skein::signal::sigterm()
        .map_err(|error| format!("failed to register SIGTERM handler: {error}"))?;
    let worker = start_worker_with_transport_and_trace_collector(
        handle,
        worker_config,
        executor,
        transport,
        trace_collector,
    );
    let signal = async move {
        std::future::poll_fn(|task_cx| {
            if sigint.poll_recv(task_cx).is_ready() || sigterm.poll_recv(task_cx).is_ready() {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        })
        .await;
    };

    // Intentionally has no timeout: if kernel cleanup is blocked, systemd's
    // service timeout and control-group kill remain the abrupt-death backstop.
    temper_worker::shutdown_worker_after_signal(
        signal,
        std::future::ready(()),
        worker,
        std::future::ready(()),
    )
    .await;
    Ok(())
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
