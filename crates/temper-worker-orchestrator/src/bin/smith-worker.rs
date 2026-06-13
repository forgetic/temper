use std::sync::Arc;

use smith_worker::{
    AgentSurface, CodingExecutor, CodingExecutorConfig, ExecutorSelection, OutOfProcessRunner,
    ParseOutcome, StubExecutor, USAGE, role_identities_from_env, run_worker,
};

fn main() {
    let outcome = smith_worker::config::parse(std::env::args().skip(1));
    match outcome {
        Ok(ParseOutcome::Help) => {
            println!("usage: {USAGE}");
        }
        Ok(ParseOutcome::Run(config)) => {
            if let Err(error) = run(config) {
                eprintln!("smith-worker: {error}");
                std::process::exit(2);
            }
        }
        Err(error) => {
            eprintln!("smith-worker: {error}\nusage: {USAGE}");
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
fn run(config: smith_worker::WorkerConfig) -> Result<(), String> {
    match config.executor.clone() {
        ExecutorSelection::Stub => {
            let executor = Arc::new(StubExecutor::success());
            smith_io_engine::block_on(async move {
                run_worker(config, executor)
                    .await
                    .map_err(|error| error.to_string())
            })
        }
        ExecutorSelection::Coding(surface) => {
            let role_identities = {
                let roles = config
                    .capabilities
                    .iter()
                    .map(|capability| capability.role.clone());
                role_identities_from_env(roles, std::env::vars())?
            };
            let executor_config = CodingExecutorConfig {
                workspace_root: surface.workspace_root,
                git_base_url: surface.git_base_url,
                role_identities,
            };

            // Both surfaces resolve to a command the out-of-process runner spawns:
            // the anvil-native surface assembles `anvil-agent` + auth/iteration flags; an
            // external command is passed through verbatim.
            let command = match surface.agent {
                AgentSurface::AnvilNative(smith) => smith.into_command(),
                AgentSurface::ExternalCommand(command) => command,
            };
            let runner = Arc::new(OutOfProcessRunner::new(command));
            // Relay agent step-progress checkpoints to the daemon (which
            // applies them to the forge idempotently); transport trouble is
            // logged and dropped, never failing the turn.
            let progress_sink = Arc::new(smith_worker::DaemonRelayProgressSink::new(
                &config.daemon_url,
                config.worker_id.clone(),
            ));
            let executor = Arc::new(
                CodingExecutor::new(executor_config, runner).with_progress_sink(progress_sink),
            );
            smith_io_engine::block_on(async move {
                run_worker(config, executor)
                    .await
                    .map_err(|error| error.to_string())
            })
        }
    }
}
