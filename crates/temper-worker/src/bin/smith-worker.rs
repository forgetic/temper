use std::sync::Arc;

use temper_worker::{
    AgentSurface, CodingExecutor, CodingExecutorConfig, ExecutorSelection, OutOfProcessRunner,
    ParseOutcome, StubExecutor, USAGE, role_identities_from_env, run_worker,
};

fn main() {
    let outcome = temper_worker::config::parse(std::env::args().skip(1));
    match outcome {
        Ok(ParseOutcome::Help) => {
            println!("usage: {USAGE}");
        }
        Ok(ParseOutcome::Run(config)) => {
            if let Err(error) = run(config) {
                eprintln!("worker: {error}");
                std::process::exit(2);
            }
        }
        Err(error) => {
            eprintln!("worker: {error}\nusage: {USAGE}");
            std::process::exit(2);
        }
    }
}

/// Builds the selected executor and runs the worker on the skein runtime.
///
/// The worker links **no** agent/LLM code: every coding job runs out-of-process
/// behind the `smith-agent-protocol`. The worker spawns the agent program
/// (the `anvil-agent` binary by default, or any operator-provided coder),
/// relaying its step-progress checkpoints. Credentials are the agent process's
/// concern — it preflights its own provider login at job start.
fn run(mut config: temper_worker::WorkerConfig) -> Result<(), String> {
    match config.executor.clone() {
        ExecutorSelection::Stub => {
            let executor = Arc::new(StubExecutor::success());
            temper_worker_io::block_on_with(move |_cx, handle| async move {
                run_worker(handle, config, executor)
                    .await
                    .map_err(|error| error.to_string())
            })
        }
        ExecutorSelection::Coding(surface) => {
            // The binary fills the worker config's role identities from the
            // environment; the executor then sources them from the config (the
            // worker's single source of truth — issue #199).
            config.role_identities = {
                let roles = config
                    .capabilities
                    .iter()
                    .map(|capability| capability.role.clone());
                role_identities_from_env(roles, std::env::vars())?
            };
            let executor_config = CodingExecutorConfig {
                workspace_root: surface.workspace_root,
                git_base_url: surface.git_base_url,
                role_identities: config.role_identities.clone(),
            };

            // Both surfaces resolve to a command the out-of-process runner spawns:
            // the temper-agent surface assembles `temper-agent` + auth/iteration flags;
            // an external command is passed through verbatim.
            let agent_surface = surface.agent;
            let command = match agent_surface {
                AgentSurface::AnvilNative(agent) => {
                    let mut command = agent.into_command();
                    command.push("--freshness-url".to_string());
                    command.push(format!(
                        "{}/v1/pr-freshness",
                        config.daemon_url.trim_end_matches('/')
                    ));
                    command
                }
                AgentSurface::ExternalCommand(command) => command,
            };
            let runner = Arc::new(OutOfProcessRunner::new(command));
            temper_worker_io::block_on_with(move |_cx, handle| async move {
                // Relay agent step-progress checkpoints to the daemon (which
                // applies them to the forge idempotently); transport trouble is
                // logged and dropped, never failing the turn. Built inside the
                // engine task so it holds the runtime handle explicitly.
                let progress_sink = Arc::new(temper_worker::DaemonRelayProgressSink::new(
                    handle.clone(),
                    &config.daemon_url,
                    config.worker_id.clone(),
                ));
                let executor = Arc::new(
                    CodingExecutor::new(executor_config, runner)
                        .with_pr_freshness_guard(Arc::new(temper_worker::HttpPrFreshnessGuard::new(
                            &config.daemon_url,
                        )))
                        .with_progress_sink(progress_sink),
                );
                run_worker(handle, config, executor)
                    .await
                    .map_err(|error| error.to_string())
            })
        }
    }
}
