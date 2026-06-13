//! Out-of-process agent runner — the production agent boundary.
//!
//! Spawns an agent **program** (the `anvil-agent` binary by default, or any
//! operator-provided coder) that speaks the `smith-agent-protocol`:
//!
//! - the worker writes the [`WorkspaceContext`] to a temp file named by
//!   [`CONTEXT_ENV`] and runs the program in the prepared checkout (cwd);
//! - the program emits [`StepProgress`] records as line-delimited JSON on its
//!   **stdout**; the worker parses each and hands it to the [`ProgressSink`] to
//!   relay onward to the forge;
//! - the program writes a [`WorkspaceResult`] to the file named by
//!   [`RESULT_ENV`], which the worker reads back.
//!
//! This replaces the former in-process pi-SDK runner: the worker links no
//! agent/LLM code, only this protocol. It also subsumes the old
//! `ExternalCommandRunner` (same file protocol) — an external coder that ignores
//! stdout simply emits no progress, which is fine.
//!
//! Spawning goes through [`skein::runtime::spawn_blocking`], never
//! `tokio::process`: the worker runs on the skein runtime, which has no
//! tokio reactor, so a blocking child must run on the blocking pool.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use smith_agent_protocol::{CONTEXT_ENV, RESULT_ENV, StepProgress, WorkspaceContext};

use crate::agent_runner::{AgentRunError, AgentRunner, ProgressSink, WorkspaceResult};

/// Spawns an agent program speaking the `smith-agent-protocol`.
#[derive(Clone, Debug)]
pub struct OutOfProcessRunner {
    /// Program followed by fixed arguments, e.g.
    /// `["anvil-agent", "--auth", "chatgpt-oauth"]`.
    command: Vec<String>,
}

impl OutOfProcessRunner {
    /// Builds a runner for the given command (program first, then args).
    pub fn new(command: Vec<String>) -> Self {
        Self { command }
    }
}

impl AgentRunner for OutOfProcessRunner {
    async fn run(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
        progress: &dyn ProgressSink,
    ) -> Result<WorkspaceResult, AgentRunError> {
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

        // The blocking child reads stdout line-by-line and pushes every parsed
        // step-progress marker into the channel *as it arrives*, so a marker the
        // agent emitted (after pushing its checkpoint) is captured even if the
        // child later dies — the crash-recovery guarantee. We drain and relay
        // after the child returns; relay latency does not affect recovery (the
        // agent already pushed + emitted before any crash).
        let (sender, mut receiver) = smith_io_engine::channel::<StepProgress>();

        let program_owned = program.clone();
        let args_owned: Vec<String> = args.to_vec();
        let cwd_owned = cwd.to_path_buf();
        let context_path_owned = context_path.clone();
        let result_path_owned = result_path.clone();
        // `skein::runtime::spawn_blocking` returns the closure's value
        // directly (no JoinError wrapper), so the closure's own
        // `Result<ChildOutcome, AgentRunError>` is what comes back.
        let outcome = skein::runtime::spawn_blocking(move || {
            run_child(
                &program_owned,
                &args_owned,
                &cwd_owned,
                &context_path_owned,
                &result_path_owned,
                &sender,
            )
        })
        .await;

        // Relay every captured marker before interpreting the exit status, so a
        // failing run still surfaces the progress it made.
        while let Some(marker) = receiver.try_recv() {
            progress.report(marker);
        }

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
        serde_json::from_slice::<WorkspaceResult>(&result_bytes).map_err(|error| {
            AgentRunError::permanent(format!("agent result file is not valid JSON: {error}"))
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

/// Runs the child to completion on the blocking pool: spawn with piped stdout +
/// stderr, stream stdout lines into `sender` as step-progress, collect stderr,
/// and return the exit outcome. Returns a [`transient`](AgentRunError::transient)
/// error only for spawn/IO failures that a re-dispatch might survive.
fn run_child(
    program: &str,
    args: &[String],
    cwd: &Path,
    context_path: &Path,
    result_path: &Path,
    sender: &smith_io_engine::CqSender<StepProgress>,
) -> Result<ChildOutcome, AgentRunError> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env(CONTEXT_ENV, context_path)
        .env(RESULT_ENV, result_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            AgentRunError::transient(format!("spawn agent command `{program}`: {error}"))
        })?;

    // Stream stdout: each non-blank line is one step-progress record. A line
    // that does not parse is ignored (forward-compatible: an older worker
    // tolerates newer/unknown lines) rather than failing the run.
    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            match StepProgress::from_line(&line) {
                Ok(Some(progress)) => {
                    // Receiver dropped ⇒ nobody is relaying; stop parsing but let
                    // the child finish (it owns the result file + pushed commits).
                    if sender.send(progress).is_err() {
                        break;
                    }
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }
    }

    let output = child.wait_with_output().map_err(|error| {
        AgentRunError::transient(format!("await agent command `{program}`: {error}"))
    })?;

    Ok(ChildOutcome {
        status_code: output.status.code(),
        stderr_tail: stderr_tail(&output.stderr, 2_000),
    })
}

/// Last `max_len` bytes of captured stderr, on a char boundary, for error
/// messages. (Secret redaction is applied by the executor, which holds the push
/// token.)
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
        let sink = crate::agent_runner::NullProgressSink;
        let outcome =
            smith_io_engine::block_on(async move { runner.run(&context, &cwd, &sink).await });
        let error = outcome.expect_err("empty command must fail");
        assert_eq!(error.class, temper_worker_protocol::FailureClass::Permanent);
    }

    fn test_context() -> WorkspaceContext {
        use smith_agent_protocol::{WorkspaceRepository, WorkspaceWorkItem};
        WorkspaceContext {
            repository: WorkspaceRepository {
                id: "acme/svc".to_string(),
                owner: "acme".to_string(),
                name: "svc".to_string(),
                default_branch: "main".to_string(),
            },
            work_item: WorkspaceWorkItem {
                role: "engineer".to_string(),
                queue: "code".to_string(),
                kind: "issue".to_string(),
                target: "Issue { number: ItemNumber(7) }".to_string(),
                context: "{}".to_string(),
            },
            base_branch: "main".to_string(),
            branch_hint: "smith/engineer/issue-7".to_string(),
            correlation_key: "pr-for-code-7".to_string(),
            checkout: Some("writable".to_string()),
            allowed_verdicts: Vec::new(),
            guidance: Default::default(),
        }
    }
}
