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

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use temper_protocol_agent::{
    PROTOCOL_VERSION, SUBMIT_FOR_PR_ADDRESS_FLAG, SubmitForPrRequest, SubmitForPrResponse,
    WorkspaceContext,
};

use crate::agent_runner::{
    AcceptedSubmitProofStore, AgentRunError, AgentRunOutput, AgentRunner, WorkspaceResult,
    handle_submit_for_pr_with_proof,
};
use crate::pre_push::submit_for_pr_pre_push_response_blocking;

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
    /// Host-controlled submit gate serviced over a worker-owned local channel
    /// while the child process remains alive.
    submit_for_pr: SubmitForPrHandler,
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
            .field("submit_for_pr", &"<handler>")
            .finish()
    }
}

impl OutOfProcessRunner {
    /// Builds a runner for the given command (program first, then args).
    pub fn new(command: Vec<String>) -> Self {
        Self {
            command,
            env: Vec::new(),
            submit_for_pr: default_submit_for_pr_handler(),
        }
    }

    /// Sets the environment injected into every spawned agent.
    #[must_use]
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
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
}

impl AgentRunner for OutOfProcessRunner {
    async fn run(
        &self,
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

        let accepted_submit = AcceptedSubmitProofStore::new();
        let program_owned = program.clone();
        let args_owned: Vec<String> = args.to_vec();
        let env_owned: Vec<(String, String)> = self.env.clone();
        let cwd_owned = cwd.to_path_buf();
        let context_owned = context.clone();
        let submit_for_pr = self.submit_for_pr.clone();
        let context_path_owned = context_path.clone();
        let result_path_owned = result_path.clone();
        let accepted_submit_for_child = accepted_submit.clone();
        // `skein::runtime::spawn_blocking` returns the closure's value
        // directly (no JoinError wrapper), so the closure's own
        // `Result<ChildOutcome, AgentRunError>` is what comes back.
        let outcome = skein::runtime::spawn_blocking(move || {
            run_child(ChildRunRequest {
                program: &program_owned,
                args: &args_owned,
                env: &env_owned,
                cwd: &cwd_owned,
                context: &context_owned,
                context_path: &context_path_owned,
                result_path: &result_path_owned,
                submit_listener,
                submit_for_pr,
                accepted_submit: accepted_submit_for_child,
            })
        })
        .await;

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
    submit_listener: Option<(TcpListener, String)>,
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
        submit_listener,
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
    let output = output?;

    Ok(ChildOutcome {
        status_code: output.status.code(),
        stderr_tail: stderr_tail(&output.stderr, 2_000),
    })
}

struct SubmitServer {
    stop: Arc<AtomicBool>,
    address: String,
    thread: Option<thread::JoinHandle<()>>,
}

impl SubmitServer {
    fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake a nonblocking accept loop promptly instead of waiting for the
        // next poll interval.
        let _ = TcpStream::connect(&self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn start_submit_server(
    listener: TcpListener,
    address: String,
    handler: SubmitForPrHandler,
    accepted_submit: AcceptedSubmitProofStore,
    context: WorkspaceContext,
    cwd: PathBuf,
) -> SubmitServer {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let thread = thread::spawn(move || {
        if listener.set_nonblocking(true).is_err() {
            return;
        }
        while !stop_for_thread.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    handle_submit_stream(stream, &handler, &accepted_submit, &context, &cwd);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    SubmitServer {
        stop,
        address,
        thread: Some(thread),
    }
}

fn handle_submit_stream(
    mut stream: TcpStream,
    handler: &SubmitForPrHandler,
    accepted_submit: &AcceptedSubmitProofStore,
    context: &WorkspaceContext,
    cwd: &Path,
) {
    let mut request_bytes = Vec::new();
    let response = match stream.read_to_end(&mut request_bytes) {
        Ok(_) => match serde_json::from_slice::<SubmitForPrRequest>(&request_bytes) {
            Ok(request) if request.protocol_version == PROTOCOL_VERSION => {
                handle_submit_for_pr_with_proof(
                    accepted_submit,
                    |request, context, cwd| handler(request, context, cwd),
                    request,
                    context,
                    cwd,
                )
            }
            Ok(request) => SubmitForPrResponse::rejected(format!(
                "submit_for_pr protocol version mismatch: got {}, expected {}",
                request.protocol_version, PROTOCOL_VERSION
            )),
            Err(error) => {
                SubmitForPrResponse::rejected(format!("invalid submit_for_pr request: {error}"))
            }
        },
        Err(error) => SubmitForPrResponse::rejected(format!("read submit_for_pr request: {error}")),
    };
    if let Ok(bytes) = serde_json::to_vec(&response) {
        let _ = stream.write_all(&bytes);
        let _ = stream.shutdown(Shutdown::Write);
    }
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
mod tests {
    use super::*;

    #[test]
    fn stderr_tail_keeps_short_input_and_truncates_long_on_boundary() {
        assert_eq!(stderr_tail(b"short", 100), "short");
        let long = "x".repeat(5_000);
        let tail = stderr_tail(long.as_bytes(), 2_000);
        assert_eq!(tail.len(), 2_000);
    }

    #[test]
    fn empty_command_is_a_permanent_error() {
        let runner = OutOfProcessRunner::new(Vec::new());
        let context = test_context();
        let cwd = std::env::temp_dir();
        let outcome = temper_worker_io::block_on(async move { runner.run(&context, &cwd).await });
        let error = outcome.expect_err("empty command must fail");
        assert_eq!(error.class, temper_protocol_worker::FailureClass::Permanent);
    }

    fn test_context() -> WorkspaceContext {
        use temper_protocol_agent::{WorkspaceRepository, WorkspaceWorkItem};
        WorkspaceContext {
            repos: vec![WorkspaceRepository {
                id: "acme/svc".to_string(),
                owner: "acme".to_string(),
                name: "svc".to_string(),
                default_branch: "main".to_string(),
                dir: "svc".to_string(),
                access: "writable".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("smith/engineer/issue-7".to_string()),
            }],
            work_item: WorkspaceWorkItem {
                role: "engineer".to_string(),
                queue: "code".to_string(),
                kind: "issue".to_string(),
                target: "Issue { number: ItemNumber(7) }".to_string(),
                context: "{}".to_string(),
            },
            action: "open_pr".to_string(),
            correlation_key: "pr-for-code-7".to_string(),
            checkout: Some("writable".to_string()),
            allowed_verdicts: Vec::new(),
            guidance: Default::default(),
            pull_request_freshness: None,
            agent_session: None,
        }
    }
}
