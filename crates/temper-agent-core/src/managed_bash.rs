//! Temper-owned process tool with joined, descendant-complete cancellation.
//!
//! Every shell is spawned through the prepared containment contract. The tool
//! future owns that containment, and normal root exit, explicit timeout,
//! operation cancellation, owner-thread failure, and Drop all converge on its
//! exactly-once cleanup proof before output can become terminal.

use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use temper_process_containment::{
    CleanupTrigger, ContainedProcess, ContainmentCommand, ContainmentScope,
};
use tongs::error::{Error, Result};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

use crate::AgentContainmentContext;

const TAIL_RING_BYTES: usize = 256 * 1024;
const MAX_OUTPUT_BYTES: usize = 50 * 1024;
const MAX_OUTPUT_LINES: usize = 2_000;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(test)]
static ACTIVE_OUTPUT_READERS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
struct ActiveReaderGuard;

#[cfg(test)]
impl ActiveReaderGuard {
    fn enter() -> Self {
        ACTIVE_OUTPUT_READERS.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

#[cfg(test)]
impl Drop for ActiveReaderGuard {
    fn drop(&mut self) {
        ACTIVE_OUTPUT_READERS.fetch_sub(1, Ordering::AcqRel);
    }
}

type ReaderThread = JoinHandle<std::io::Result<Tail>>;

pub struct ManagedBashTool {
    cwd: PathBuf,
    containment: AgentContainmentContext,
}

impl ManagedBashTool {
    /// Standalone convenience constructor. Agent composition roots should use
    /// [`Self::with_containment`] so every nested owner shares one concrete
    /// context and tests can force a backend without global state.
    pub fn new(cwd: &Path) -> Self {
        Self::with_containment(cwd, AgentContainmentContext::production(None))
    }

    pub fn with_containment(cwd: &Path, containment: AgentContainmentContext) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            containment,
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
        tool_call_id: &str,
        input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> Result<ToolOutput> {
        let input: BashInput = serde_json::from_value(input)
            .map_err(|error| Error::tool("bash", format!("invalid input: {error}")))?;
        let timeout = input.timeout.filter(|seconds| *seconds > 0);
        let mut task = ManagedBashTask::spawn(
            self.cwd.clone(),
            tool_call_id,
            input,
            self.containment.clone(),
        )?;
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

/// A shell future that directly owns its prepared process boundary. The owner
/// thread may poll the root and render output, but neither it nor this future
/// can publish a result until `ContainedProcess::cleanup` has returned its
/// recursive-empty report and the output reader has been joined.
struct ManagedBashTask {
    state: Arc<Mutex<TaskState>>,
    cancelled: Arc<AtomicBool>,
    timed_out: Arc<AtomicBool>,
    process: Arc<ContainedProcess>,
    reader: Arc<Mutex<Option<ReaderThread>>>,
    thread: Option<JoinHandle<()>>,
}

impl ManagedBashTask {
    fn spawn(
        cwd: PathBuf,
        tool_call_id: &str,
        input: BashInput,
        containment: AgentContainmentContext,
    ) -> Result<Self> {
        let spec = containment.containment_spec(tool_call_id, ContainmentScope::Tool);
        let prepared = containment
            .factory()
            .prepare(spec)
            .map_err(|error| Error::tool("bash", format!("prepare containment: {error}")))?;
        let mut command = managed_command(&input.command);
        command
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let process = Arc::new(
            prepared
                .spawn(command)
                .map_err(|error| Error::tool("bash", format!("spawn failed: {error}")))?,
        );
        let stdout = process
            .take_stdout()
            .map_err(|error| Error::tool("bash", format!("take child stdout: {error}")))?
            .ok_or_else(|| Error::tool("bash", "child stdout unavailable"));
        let stdout = match stdout {
            Ok(stdout) => stdout,
            Err(error) => {
                let _report = process.cleanup(CleanupTrigger::Shutdown);
                return Err(error);
            }
        };
        let output_reader = thread::Builder::new()
            .name(format!("temper-bash-output-{}", process.id()))
            .spawn(move || {
                #[cfg(test)]
                let _active_reader = ActiveReaderGuard::enter();
                read_tail(stdout)
            });
        let output_reader = match output_reader {
            Ok(reader) => reader,
            Err(error) => {
                let _report = process.cleanup(CleanupTrigger::Shutdown);
                return Err(Error::tool("bash", format!("start output reader: {error}")));
            }
        };

        let state = Arc::new(Mutex::new(TaskState {
            result: None,
            waker: None,
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let timed_out = Arc::new(AtomicBool::new(false));
        let reader = Arc::new(Mutex::new(Some(output_reader)));
        let thread_state = Arc::clone(&state);
        let thread_cancelled = Arc::clone(&cancelled);
        let thread_timed_out = Arc::clone(&timed_out);
        let thread_process = Arc::clone(&process);
        let thread_reader = Arc::clone(&reader);
        let requested_timeout = input.timeout.filter(|seconds| *seconds > 0);
        let owner = thread::Builder::new()
            .name("temper-managed-bash".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_command(
                        &thread_process,
                        &thread_reader,
                        &thread_cancelled,
                        &thread_timed_out,
                        requested_timeout,
                    )
                }))
                .unwrap_or_else(|_| {
                    let _report = thread_process.cleanup(CleanupTrigger::Cancellation);
                    let _ = join_reader(&thread_reader);
                    Err(Error::tool("bash", "command owner thread failed"))
                });
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
            });
        let owner = match owner {
            Ok(owner) => owner,
            Err(error) => {
                let _report = process.cleanup(CleanupTrigger::Shutdown);
                let _ = join_reader(&reader);
                return Err(Error::tool("bash", format!("start command owner: {error}")));
            }
        };

        Ok(Self {
            state,
            cancelled,
            timed_out,
            process,
            reader,
            thread: Some(owner),
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
        // The owner normally joins the reader while constructing the result.
        // This second exactly-once take covers an owner-thread panic.
        let _ = join_reader(&self.reader);
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
            None if self
                .thread
                .as_ref()
                .is_some_and(|owner| owner.is_finished()) =>
            {
                // A panic before the owner could publish still takes the same
                // cleanup gate and reader join before becoming terminal.
                let _report = self.process.cleanup(CleanupTrigger::Cancellation);
                self.join();
                Poll::Ready(Err(Error::tool("bash", "command owner thread failed")))
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for ManagedBashTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        // The dedicated command owner holds the process and reader Arcs, runs
        // proof-based cleanup, and joins output. Detaching its JoinHandle here
        // prevents a blocked containment inspection from starving standalone's
        // event loop; the attempt-owned emergency registry remains registered
        // until cleanup itself completes.
        let _ = self.thread.take();
    }
}

enum RootOutcome {
    Exited(ExitStatus),
    Cancelled,
    WaitFailed(std::io::Error),
}

fn run_command(
    process: &ContainedProcess,
    reader: &Arc<Mutex<Option<ReaderThread>>>,
    cancelled: &AtomicBool,
    timed_out: &AtomicBool,
    requested_timeout: Option<u64>,
) -> Result<ToolOutput> {
    let outcome = loop {
        if cancelled.load(Ordering::Acquire) {
            break RootOutcome::Cancelled;
        }
        match process.try_wait_root() {
            Ok(Some(status)) => break RootOutcome::Exited(status),
            Ok(None) => thread::sleep(PROCESS_POLL_INTERVAL),
            Err(error) => break RootOutcome::WaitFailed(error),
        }
    };

    let command_timed_out = timed_out.load(Ordering::Acquire);
    let trigger = match outcome {
        RootOutcome::Exited(_) => CleanupTrigger::NormalRootExit,
        RootOutcome::Cancelled if command_timed_out => CleanupTrigger::Timeout,
        RootOutcome::Cancelled | RootOutcome::WaitFailed(_) => CleanupTrigger::Cancellation,
    };
    // This is the completion gate. A report can only be constructed after the
    // direct child is reaped and recursive emptiness is independently proven.
    let _cleanup_report = process.cleanup(trigger);
    let tail = join_reader(reader)?;

    match outcome {
        RootOutcome::WaitFailed(error) => {
            return Err(Error::tool("bash", format!("waiting for exit: {error}")));
        }
        RootOutcome::Cancelled if !command_timed_out => {
            return Ok(ToolOutput::error("bash command cancelled"));
        }
        RootOutcome::Exited(_) | RootOutcome::Cancelled => {}
    }

    let text = String::from_utf8_lossy(&tail.bytes).into_owned();
    let exit_code = match &outcome {
        RootOutcome::Exited(status) => status.code(),
        RootOutcome::Cancelled | RootOutcome::WaitFailed(_) => None,
    };
    let (rendered, is_error) = render_outcome(
        &text,
        tail.dropped,
        exit_code,
        command_timed_out.then_some(requested_timeout.unwrap_or_default()),
    );
    Ok(ToolOutput {
        is_error,
        ..ToolOutput::text(rendered)
    })
}

fn join_reader(reader: &Arc<Mutex<Option<ReaderThread>>>) -> Result<Tail> {
    let reader = reader
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let Some(reader) = reader else {
        return Err(Error::tool(
            "bash",
            "output reader was unavailable before terminal completion",
        ));
    };
    reader
        .join()
        .map_err(|_| Error::tool("bash", "output reader panicked"))?
        .map_err(|error| Error::tool("bash", format!("reading output: {error}")))
}

fn managed_command(script: &str) -> ContainmentCommand {
    #[cfg(unix)]
    {
        let mut command = ContainmentCommand::new("sh");
        command
            .arg("-c")
            .arg("\"$@\"")
            .arg("temper-managed-bash")
            .arg("bash")
            .arg("-c")
            .arg(format!("exec 2>&1\n{script}"));
        command
    }
    #[cfg(not(unix))]
    {
        let mut command = ContainmentCommand::new("bash");
        command.arg("-c").arg(format!("exec 2>&1\n{script}"));
        command
    }
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
mod tests;
