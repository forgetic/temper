//! Temper-owned process tool with joined, descendant-aware cancellation.
//!
//! Tongs' bash tool intentionally only kills its direct child. Agent operation
//! deadlines, however, drop the tool future, so the process owner itself must
//! make Drop a complete cleanup boundary. `ManagedBashTask` owns a joined
//! command thread; cancellation kills the command's isolated containment group,
//! reaps the shell, closes inherited output pipes, and joins the reader before
//! returning from Drop.

use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use temper_process_containment::{ProcessContainment, configure_descendant_command};
use tongs::error::{Error, Result};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

const TAIL_RING_BYTES: usize = 256 * 1024;
const MAX_OUTPUT_BYTES: usize = 50 * 1024;
const MAX_OUTPUT_LINES: usize = 2_000;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub struct ManagedBashTool {
    cwd: PathBuf,
}

impl ManagedBashTool {
    pub fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
        }
    }
}

#[derive(Deserialize)]
struct BashInput {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
}

#[async_trait]
impl Tool for ManagedBashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command in the workspace. stdout and stderr are merged; \
         long output is truncated to the most recent lines."
    }

    fn parameters(&self) -> serde_json::Value {
        // Keep the provider contract byte-for-byte equivalent to tongs' bash
        // definition so replacing its implementation is invisible to models.
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Bash command to execute"
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds (optional, no default timeout)"
                }
            },
            "required": ["command"]
        })
    }

    fn effects(&self) -> ToolEffects {
        ToolEffects::process()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> Result<ToolOutput> {
        let input: BashInput = serde_json::from_value(input)
            .map_err(|error| Error::tool("bash", format!("invalid input: {error}")))?;
        let timeout = input.timeout.filter(|seconds| *seconds > 0);
        let mut task = ManagedBashTask::spawn(self.cwd.clone(), input)?;
        let Some(seconds) = timeout else {
            return task.await;
        };
        match temper_agent_io::timeout(Duration::from_secs(seconds), &mut task).await {
            Ok(result) => result,
            Err(_) => {
                task.timeout();
                task.await
            }
        }
    }
}

struct TaskState {
    result: Option<Result<ToolOutput>>,
    waker: Option<Waker>,
}

/// Future whose Drop requests process-tree cancellation and synchronously joins
/// all blocking work. Consequently the agent task-group's quiescence boundary
/// cannot race a leftover shell, grandchild, reader, or wait thread.
struct ManagedBashTask {
    state: Arc<Mutex<TaskState>>,
    cancelled: Arc<AtomicBool>,
    timed_out: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ManagedBashTask {
    fn spawn(cwd: PathBuf, input: BashInput) -> Result<Self> {
        let state = Arc::new(Mutex::new(TaskState {
            result: None,
            waker: None,
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let timed_out = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_cancelled = Arc::clone(&cancelled);
        let thread_timed_out = Arc::clone(&timed_out);
        let thread = thread::Builder::new()
            .name("temper-managed-bash".to_string())
            .spawn(move || {
                let result = run_command(cwd, input, &thread_cancelled, &thread_timed_out);
                let waker = {
                    let mut state = thread_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.result = Some(result);
                    state.waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            })
            .map_err(|error| Error::tool("bash", format!("start command owner: {error}")))?;
        Ok(Self {
            state,
            cancelled,
            timed_out,
            thread: Some(thread),
        })
    }

    fn timeout(&self) {
        self.timed_out.store(true, Ordering::Release);
        self.cancelled.store(true, Ordering::Release);
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Future for ManagedBashTask {
    type Output = Result<ToolOutput>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.result.is_none()
                && !state
                    .waker
                    .as_ref()
                    .is_some_and(|waker| waker.will_wake(cx.waker()))
            {
                state.waker = Some(cx.waker().clone());
            }
            state.result.take()
        };
        match result {
            Some(result) => {
                self.join();
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for ManagedBashTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.join();
    }
}

fn run_command(
    cwd: PathBuf,
    input: BashInput,
    cancelled: &AtomicBool,
    timed_out: &AtomicBool,
) -> Result<ToolOutput> {
    let mut command = managed_command(&input.command);
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure_descendant_command(&mut command);

    let mut child = command
        .spawn()
        .map_err(|error| Error::tool("bash", format!("spawn failed: {error}")))?;
    let containment = ProcessContainment::attach(&child).map_err(|error| {
        let _ = child.kill();
        let _ = child.wait();
        Error::tool("bash", format!("attach process containment: {error}"))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::tool("bash", "child stdout unavailable"))?;
    let reader = thread::Builder::new()
        .name(format!("temper-bash-output-{}", child.id()))
        .spawn(move || read_tail(stdout))
        .map_err(|error| {
            let _ = containment.hard_kill(&mut child);
            let _ = child.wait();
            Error::tool("bash", format!("start output reader: {error}"))
        })?;

    let timeout = input.timeout.filter(|seconds| *seconds > 0);
    let (status, command_timed_out) = loop {
        if cancelled.load(Ordering::Acquire) {
            kill_and_reap(&containment, &mut child);
            break (None, timed_out.load(Ordering::Acquire));
        }
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) => thread::sleep(PROCESS_POLL_INTERVAL),
            Err(error) => {
                kill_and_reap(&containment, &mut child);
                let _ = reader.join();
                return Err(Error::tool("bash", format!("waiting for exit: {error}")));
            }
        }
    };

    // A direct shell may exit while a background grandchild still owns its
    // stdout. Empty the isolated subtree before joining the reader.
    let _ = containment.hard_kill(&mut child);
    let _ = child.wait();
    let tail = reader
        .join()
        .map_err(|_| Error::tool("bash", "output reader panicked"))?
        .map_err(|error| Error::tool("bash", format!("reading output: {error}")))?;

    if cancelled.load(Ordering::Acquire) && !command_timed_out {
        return Ok(ToolOutput::error("bash command cancelled"));
    }
    let text = String::from_utf8_lossy(&tail.bytes).into_owned();
    let (rendered, is_error) = render_outcome(
        &text,
        tail.dropped,
        status.as_ref().and_then(ExitStatus::code),
        command_timed_out.then_some(timeout.unwrap_or_default()),
    );
    Ok(ToolOutput {
        is_error,
        ..ToolOutput::text(rendered)
    })
}

fn managed_command(script: &str) -> Command {
    #[cfg(unix)]
    {
        // Keep the parent-death trap outside the model-controlled script so the
        // script cannot replace the relay that protects abrupt attempt death.
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("trap 'trap - TERM; kill -KILL -- -$$' TERM; \"$@\"")
            .arg("temper-managed-bash")
            .arg("bash")
            .arg("-c")
            .arg(format!("exec 2>&1\n{script}"));
        command
    }
    #[cfg(not(unix))]
    {
        let mut command = Command::new("bash");
        command.arg("-c").arg(format!("exec 2>&1\n{script}"));
        command
    }
}

fn kill_and_reap(containment: &ProcessContainment, child: &mut std::process::Child) {
    let _ = containment.hard_kill(child);
    let _ = child.kill();
    let _ = child.wait();
}

struct Tail {
    bytes: Vec<u8>,
    dropped: bool,
}

fn read_tail(mut reader: impl std::io::Read) -> std::io::Result<Tail> {
    let mut bytes = VecDeque::new();
    let mut dropped = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(Tail {
                bytes: bytes.into(),
                dropped,
            });
        }
        bytes.extend(&buffer[..read]);
        while bytes.len() > TAIL_RING_BYTES {
            bytes.pop_front();
            dropped = true;
        }
    }
}

fn render_outcome(
    text: &str,
    ring_dropped: bool,
    exit_code: Option<i32>,
    timed_out: Option<u64>,
) -> (String, bool) {
    let (content, output_lines, truncated) = truncate_tail(text);
    let mut output = String::new();
    if truncated || ring_dropped {
        output.push_str(&format!(
            "[Output truncated: showing the last {output_lines} lines]\n"
        ));
    }
    output.push_str(&content);
    if output.is_empty() {
        output.push_str("(no output)");
    }
    if let Some(seconds) = timed_out {
        output.push_str(&format!("\n\nCommand timed out after {seconds} seconds"));
        return (output, true);
    }
    match exit_code {
        Some(0) => (output, false),
        Some(code) => {
            output.push_str(&format!("\n\nExit code: {code}"));
            (output, true)
        }
        None => {
            output.push_str("\n\nCommand terminated by signal");
            (output, true)
        }
    }
}

fn truncate_tail(content: &str) -> (String, usize, bool) {
    let mut lines = content.split('\n').collect::<Vec<_>>();
    if content.ends_with('\n') {
        lines.pop();
    }
    if lines.len() <= MAX_OUTPUT_LINES && content.len() <= MAX_OUTPUT_BYTES {
        return (content.to_string(), lines.len(), false);
    }

    let mut output = Vec::new();
    let mut bytes = 0_usize;
    for (taken, line) in lines.iter().rev().enumerate() {
        if taken >= MAX_OUTPUT_LINES {
            break;
        }
        let line_bytes = line.len() + usize::from(taken > 0);
        if bytes.saturating_add(line_bytes) > MAX_OUTPUT_BYTES {
            break;
        }
        output.push(*line);
        bytes += line_bytes;
    }
    output.reverse();
    (output.join("\n"), output.len(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command as StdCommand, Stdio as StdStdio};

    fn text(output: &ToolOutput) -> &str {
        match &output.content[0] {
            tongs::model::ContentBlock::Text(text) => &text.text,
            other => panic!("expected text output, got {other:?}"),
        }
    }

    #[test]
    fn schema_and_success_output_preserve_the_tongs_contract() {
        let dir = tempfile::tempdir().expect("tempdir");
        let managed = ManagedBashTool::new(dir.path());
        let tongs = tongs::tools::create_bash_tool(dir.path());
        assert_eq!(managed.name(), tongs.name());
        assert_eq!(managed.description(), tongs.description());
        assert_eq!(managed.parameters(), tongs.parameters());
        assert_eq!(managed.effects(), tongs.effects());

        let output = temper_agent_io::block_on(async move {
            managed
                .execute(
                    "call",
                    serde_json::json!({"command": "printf 'hello\\n'"}),
                    None,
                )
                .await
                .expect("managed bash")
        });
        assert_eq!(text(&output), "hello\n");
        assert!(!output.is_error);
    }

    #[test]
    #[cfg(unix)]
    fn dropping_a_hung_command_reaps_its_grandchild_and_joins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("grandchild.pid");
        let command = format!("sleep 60 & echo $! > '{}'; wait", pid_file.display());
        let tool = ManagedBashTool::new(dir.path());
        let timed_out = temper_agent_io::block_on(async move {
            temper_agent_io::timeout(
                Duration::from_millis(100),
                tool.execute("hung", serde_json::json!({"command": command}), None),
            )
            .await
        });
        assert!(timed_out.is_err(), "generic operation timeout must win");

        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("grandchild pid was published")
            .trim()
            .parse()
            .expect("numeric pid");
        assert!(!process_alive(pid), "grandchild {pid} survived tool drop");
    }

    #[test]
    fn output_is_tail_bounded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = ManagedBashTool::new(dir.path());
        let output = temper_agent_io::block_on(async move {
            tool.execute("large", serde_json::json!({"command": "seq 1 20000"}), None)
                .await
                .expect("managed bash")
        });
        let output = text(&output);
        assert!(output.len() <= MAX_OUTPUT_BYTES + 100);
        assert!(output.contains("20000"));
        assert!(!output.contains("\n1\n"));
    }

    #[cfg(unix)]
    fn process_alive(pid: u32) -> bool {
        let exists = StdCommand::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stderr(StdStdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !exists {
            return false;
        }
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| stat.rsplit_once(") ").map(|(_, rest)| rest.to_string()))
            .and_then(|rest| rest.chars().next())
            .is_some_and(|state| state != 'Z')
    }
}
