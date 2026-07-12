//! Out-of-process agent runner — the production agent boundary.
//!
//! Spawns an agent **program** (the `temper-agent` binary by default, or any
//! operator-provided coder) that speaks the `smith-agent-protocol`:
//!
//! - the worker writes the [`WorkspaceContext`] to a temp file and passes its
//!   path as the `--context` flag, the result path as `--result`, and the
//!   prepared coordination-scoped workspace root as `--workspace` (also the
//!   child's cwd);
//! - the program writes a [`WorkspaceResult`] to the file named by `--result`,
//!   which the worker reads back.
//!
//! This replaces the former in-process pi-SDK runner: the worker links no
//! agent/LLM code, only this protocol. It also subsumes the old
//! `ExternalCommandRunner` (same file protocol).
//!
//! Spawning goes through [`skein::runtime::spawn_blocking`], never
//! `tokio::process`: the worker runs on the skein runtime, which has no
//! tokio reactor, so a blocking child must run on the blocking pool.

use std::future::Future;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use temper_protocol_agent::{
    AgentToolConfig, FORGE_CONTEXT_ADDRESS_FLAG, ForgeContextResponse, SUBMIT_FOR_PR_ADDRESS_FLAG,
    SubmitForPrRequest, SubmitForPrResponse, TOOL_CONFIG_FLAG, WorkspaceContext,
};

use crate::agent_runner::{
    AcceptedSubmitProofStore, AgentForgeContextHost, AgentRunError, AgentRunOutput, AgentRunner,
    WorkspaceResult,
};
use crate::pre_push::submit_for_pr_pre_push_response_blocking;

mod side_channel;
use side_channel::{ForgeSideChannelRequest, start_forge_server, start_submit_server};

/// Host-side submit gate used by the out-of-process carrier.
type SubmitForPrHandler =
    Arc<dyn Fn(SubmitForPrRequest, &WorkspaceContext, &Path) -> SubmitForPrResponse + Send + Sync>;

fn default_submit_for_pr_handler() -> SubmitForPrHandler {
    Arc::new(|request, context, cwd| {
        submit_for_pr_pre_push_response_blocking(request, context, cwd)
    })
}

/// Spawns an agent program speaking the `smith-agent-protocol`.
#[derive(Clone)]
pub struct OutOfProcessRunner {
    /// Program followed by fixed arguments, e.g.
    /// `["temper", "agent", "--provider", "anthropic", "--model", "…"]`. The
    /// per-job `--context`/`--result`/`--workspace` flags are appended at spawn.
    command: Vec<String>,
    /// Environment injected into every spawned agent (on top of the inherited
    /// environment): just the one secret provider-credential var, which a
    /// config-driven worker passes explicitly rather than relying on its own
    /// inherited environment.
    env: Vec<(String, String)>,
    /// Non-secret agent-local tool settings. When present and enabled for the
    /// current workflow role, these are written to a per-run JSON file and
    /// passed as `--tool-config <file>`.
    tool_config: Option<AgentToolConfig>,
    /// Host-controlled submit gate serviced over a worker-owned local channel
    /// while the child process remains alive.
    submit_for_pr: SubmitForPrHandler,
    /// Optional authenticated, assignment-bound read-only Forge host.
    forge_context: Option<AgentForgeContextHost>,
}

impl std::fmt::Debug for OutOfProcessRunner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OutOfProcessRunner")
            .field("command", &self.command)
            .field(
                "env",
                &self.env.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            )
            .field("tool_config", &self.tool_config)
            .field("submit_for_pr", &"<handler>")
            .field(
                "forge_context",
                &self.forge_context.as_ref().map(|_| "<host>"),
            )
            .finish()
    }
}

impl OutOfProcessRunner {
    /// Builds a runner for the given command (program first, then args).
    pub fn new(command: Vec<String>) -> Self {
        Self {
            command,
            env: Vec::new(),
            tool_config: None,
            submit_for_pr: default_submit_for_pr_handler(),
            forge_context: None,
        }
    }

    /// Sets the environment injected into every spawned agent.
    #[must_use]
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    /// Sets the non-secret agent tool config written per run when enabled for
    /// the assigned workflow role.
    #[must_use]
    pub fn with_tool_config(mut self, tool_config: Option<AgentToolConfig>) -> Self {
        self.tool_config = tool_config;
        self
    }

    /// Overrides the host-controlled `submit_for_pr` gate serviced for writable
    /// engineer sessions.
    #[must_use]
    pub fn with_submit_for_pr_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(SubmitForPrRequest, &WorkspaceContext, &Path) -> SubmitForPrResponse
            + Send
            + Sync
            + 'static,
    {
        self.submit_for_pr = Arc::new(handler);
        self
    }

    /// Installs the worker-owned assignment-bound Forge read host.
    #[must_use]
    pub fn with_forge_context_host(mut self, host: AgentForgeContextHost) -> Self {
        self.forge_context = Some(host);
        self
    }
}

impl AgentRunner for OutOfProcessRunner {
    async fn run(
        &self,
        job_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> Result<AgentRunOutput, AgentRunError> {
        let Some((program, args)) = self.command.split_first() else {
            return Err(AgentRunError::permanent("agent command is empty"));
        };

        let temp = tempfile::tempdir()
            .map_err(|error| AgentRunError::transient(format!("create agent temp dir: {error}")))?;
        let context_path = temp.path().join("context.json");
        let result_path = temp.path().join("result.json");

        let context_bytes = serde_json::to_vec_pretty(context).map_err(|error| {
            AgentRunError::transient(format!("serialize agent context: {error}"))
        })?;
        std::fs::write(&context_path, context_bytes).map_err(|error| {
            AgentRunError::transient(format!("write agent context file: {error}"))
        })?;

        let tool_config_path = match self
            .tool_config
            .as_ref()
            .filter(|config| config.enabled_for_role(&context.work_item.role))
        {
            Some(tool_config) => {
                let path = temp.path().join("tool-config.json");
                let bytes = serde_json::to_vec_pretty(tool_config).map_err(|error| {
                    AgentRunError::transient(format!("serialize agent tool config: {error}"))
                })?;
                std::fs::write(&path, bytes).map_err(|error| {
                    AgentRunError::transient(format!("write agent tool config file: {error}"))
                })?;
                Some(path)
            }
            None => None,
        };

        let submit_listener = if submit_for_pr_available(context) {
            let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
                AgentRunError::transient(format!("bind submit_for_pr side channel: {error}"))
            })?;
            let address = listener.local_addr().map_err(|error| {
                AgentRunError::transient(format!(
                    "read submit_for_pr side-channel address: {error}"
                ))
            })?;
            Some((listener, address.to_string()))
        } else {
            None
        };

        let forge_listener = if self.forge_context.is_some() {
            let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
                AgentRunError::transient(format!("bind Forge context side channel: {error}"))
            })?;
            let address = listener.local_addr().map_err(|error| {
                AgentRunError::transient(format!(
                    "read Forge context side-channel address: {error}"
                ))
            })?;
            Some((listener, address.to_string()))
        } else {
            None
        };

        let accepted_submit = AcceptedSubmitProofStore::new();
        let program_owned = program.clone();
        let args_owned: Vec<String> = args.to_vec();
        let env_owned: Vec<(String, String)> = self.env.clone();
        let cwd_owned = cwd.to_path_buf();
        let context_owned = context.clone();
        let submit_for_pr = self.submit_for_pr.clone();
        let context_path_owned = context_path.clone();
        let result_path_owned = result_path.clone();
        let tool_config_path_owned = tool_config_path.clone();
        let accepted_submit_for_child = accepted_submit.clone();
        let (forge_requests, mut forge_request_rx) = temper_worker_io::channel();
        let forge_context = self.forge_context.clone();
        let job_id = job_id.to_string();
        // `skein::runtime::spawn_blocking` returns the closure's value
        // directly (no JoinError wrapper), so the closure's own
        // `Result<ChildOutcome, AgentRunError>` is what comes back.
        let mut child = Box::pin(skein::runtime::spawn_blocking(move || {
            run_child(ChildRunRequest {
                program: &program_owned,
                args: &args_owned,
                env: &env_owned,
                cwd: &cwd_owned,
                context: &context_owned,
                context_path: &context_path_owned,
                result_path: &result_path_owned,
                tool_config_path: tool_config_path_owned.as_deref(),
                submit_listener,
                forge_listener,
                forge_requests,
                submit_for_pr,
                accepted_submit: accepted_submit_for_child,
            })
        }));
        let outcome = loop {
            enum Next {
                Child(Result<ChildOutcome, AgentRunError>),
                Forge(Option<ForgeSideChannelRequest>),
            }
            let next = std::future::poll_fn(|task_cx| {
                if let std::task::Poll::Ready(outcome) = child.as_mut().poll(task_cx) {
                    return std::task::Poll::Ready(Next::Child(outcome));
                }
                if forge_context.is_some() {
                    let mut receive = Box::pin(forge_request_rx.recv());
                    match receive.as_mut().poll(task_cx) {
                        std::task::Poll::Ready(request) => {
                            std::task::Poll::Ready(Next::Forge(request))
                        }
                        std::task::Poll::Pending => std::task::Poll::Pending,
                    }
                } else {
                    std::task::Poll::Pending
                }
            })
            .await;
            match next {
                Next::Child(outcome) => break outcome,
                Next::Forge(Some(request)) => {
                    let response = match &forge_context {
                        Some(host) => match host(job_id.clone(), request.operation).await {
                            Ok(result) => ForgeContextResponse::success(result),
                            Err(code) => ForgeContextResponse::error(code),
                        },
                        None => ForgeContextResponse::error(
                            temper_protocol_agent::ForgeContextErrorCode::NotAuthorized,
                        ),
                    };
                    let _ = request.response.send(response);
                }
                Next::Forge(None) => break child.as_mut().await,
            }
        };

        let ChildOutcome {
            status_code,
            stderr_tail,
        } = outcome?;
        if let Some(code) = status_code {
            if code != 0 {
                return Err(AgentRunError::transient(format!(
                    "agent command exited with status {code}; stderr tail: {stderr_tail}"
                )));
            }
        } else {
            return Err(AgentRunError::transient(format!(
                "agent command terminated without an exit code; stderr tail: {stderr_tail}"
            )));
        }

        let result_bytes = std::fs::read(&result_path).map_err(|error| {
            AgentRunError::permanent(format!("agent did not write a valid result file: {error}"))
        })?;
        let result = serde_json::from_slice::<WorkspaceResult>(&result_bytes).map_err(|error| {
            AgentRunError::permanent(format!("agent result file is not valid JSON: {error}"))
        })?;
        Ok(AgentRunOutput {
            result,
            accepted_submit: accepted_submit.latest(),
        })
    }
}

/// What the blocking child run produced.
struct ChildOutcome {
    /// Process exit code (`None` if terminated by signal without a code).
    status_code: Option<i32>,
    /// Last bytes of captured stderr, for error messages.
    stderr_tail: String,
}

struct ChildRunRequest<'a> {
    program: &'a str,
    args: &'a [String],
    env: &'a [(String, String)],
    cwd: &'a Path,
    context: &'a WorkspaceContext,
    context_path: &'a Path,
    result_path: &'a Path,
    tool_config_path: Option<&'a Path>,
    submit_listener: Option<(TcpListener, String)>,
    forge_listener: Option<(TcpListener, String)>,
    forge_requests: temper_worker_io::CqSender<ForgeSideChannelRequest>,
    submit_for_pr: SubmitForPrHandler,
    accepted_submit: AcceptedSubmitProofStore,
}

/// Runs the child to completion on the blocking pool: spawn with stdout
/// discarded, collect stderr, and return the exit outcome. Returns a
/// [`transient`](AgentRunError::transient) error only for spawn/IO failures that
/// a re-dispatch might survive.
fn run_child(request: ChildRunRequest<'_>) -> Result<ChildOutcome, AgentRunError> {
    let ChildRunRequest {
        program,
        args,
        env,
        cwd,
        context,
        context_path,
        result_path,
        tool_config_path,
        submit_listener,
        forge_listener,
        forge_requests,
        submit_for_pr,
        accepted_submit,
    } = request;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        // The context/result/workspace paths are per-job flags (the workspace is
        // also the child's cwd). Passing them as flags — not env — keeps the
        // agent's only env input the one secret credential var.
        .arg("--context")
        .arg(context_path)
        .arg("--result")
        .arg(result_path)
        .arg("--workspace")
        .arg(cwd);
    if let Some(path) = tool_config_path {
        command.arg(TOOL_CONFIG_FLAG).arg(path);
    }
    let submit_server = submit_listener.map(|(listener, address)| {
        command.arg(SUBMIT_FOR_PR_ADDRESS_FLAG).arg(&address);
        start_submit_server(
            listener,
            address,
            submit_for_pr,
            accepted_submit,
            context.clone(),
            cwd.to_path_buf(),
        )
    });
    let forge_server = forge_listener.map(|(listener, address)| {
        command.arg(FORGE_CONTEXT_ADDRESS_FLAG).arg(&address);
        start_forge_server(listener, address, forge_requests)
    });
    // Inject the one secret (the provider credential) explicitly, so the agent
    // does not depend on the worker's own inherited environment.
    for (key, value) in env {
        command.env(key, value);
    }
    let output = match command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child.wait_with_output().map_err(|error| {
            AgentRunError::transient(format!("wait for agent command `{program}`: {error}"))
        }),
        Err(error) => Err(AgentRunError::transient(format!(
            "spawn agent command `{program}`: {error}"
        ))),
    };
    if let Some(server) = submit_server {
        server.stop();
    }
    if let Some(server) = forge_server {
        server.stop();
    }
    let output = output?;

    Ok(ChildOutcome {
        status_code: output.status.code(),
        stderr_tail: stderr_tail(&output.stderr, 2_000),
    })
}

fn submit_for_pr_available(context: &WorkspaceContext) -> bool {
    context.work_item.role == "engineer"
        && context.repos.iter().any(|repo| repo.is_writable())
        && !matches!(
            context.checkout.as_deref(),
            Some("read_only" | "pull_request_read_only")
        )
}

/// Last `max_len` bytes of captured stderr, on a char boundary, for error
/// messages. The push token is never embedded in a command label or remote URL
/// (it is passed via a separate `-c http.extraheader` arg), so captured stderr
/// does not carry it.
fn stderr_tail(stderr: &[u8], max_len: usize) -> String {
    let text = String::from_utf8_lossy(stderr).into_owned();
    if text.len() <= max_len {
        return text;
    }
    let mut start = text.len() - max_len;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

#[cfg(test)]
#[path = "out_of_process_runner_tests.rs"]
mod tests;
