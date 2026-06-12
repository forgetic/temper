// SPDX-License-Identifier: MPL-2.0

//! Child-process requests: spawn, feed stdin, await output with a deadline.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use skein::cx::Cx;
use skein::io::AsyncWriteExt;
use skein::process::{Command, Stdio};
use skein::time::timeout;

/// One child-process run, as data.
#[derive(Debug, Clone)]
pub struct ProcessCall {
    pub program: String,
    pub args: Vec<String>,
    /// Start from an empty environment instead of inheriting the parent's.
    pub clear_env: bool,
    pub env: BTreeMap<String, String>,
    pub current_dir: Option<PathBuf>,
    /// Bytes written to the child's stdin before it is closed. `None` leaves
    /// stdin disconnected (null).
    pub stdin: Option<Vec<u8>>,
    /// Kill the child and fail if it has not exited within this window.
    pub timeout: Option<Duration>,
}

impl ProcessCall {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            clear_env: false,
            env: BTreeMap::new(),
            current_dir: None,
            stdin: None,
            timeout: None,
        }
    }
}

/// Outcome of a [`ProcessCall`].
#[derive(Debug)]
pub struct ProcessOutput {
    pub status_code: Option<i32>,
    /// Human-readable exit status (e.g. `exit status: 1`).
    pub status_display: String,
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Errors from running a child process.
#[derive(Debug)]
pub enum ProcessCallError {
    Spawn(String),
    Io {
        operation: &'static str,
        message: String,
    },
    TimedOut,
}

impl std::fmt::Display for ProcessCallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessCallError::Spawn(error) => write!(formatter, "failed to spawn process: {error}"),
            ProcessCallError::Io { operation, message } => {
                write!(formatter, "process {operation} failed: {message}")
            }
            ProcessCallError::TimedOut => write!(formatter, "process timed out"),
        }
    }
}

impl std::error::Error for ProcessCallError {}

/// Run one child process to completion on the engine runtime.
pub async fn run_process(cx: &Cx, call: ProcessCall) -> Result<ProcessOutput, ProcessCallError> {
    let mut command = Command::new(&call.program);
    // If the wait future is dropped because the timeout elapsed, the child
    // must not outlive the request.
    command.kill_on_drop(true);
    for arg in &call.args {
        command.arg(arg);
    }
    if call.clear_env {
        command.env_clear();
    }
    for (key, value) in &call.env {
        command.env(key, value);
    }
    if let Some(dir) = &call.current_dir {
        command.current_dir(dir);
    }
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.stdin(if call.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });

    let mut child = command
        .spawn()
        .map_err(|error| ProcessCallError::Spawn(error.to_string()))?;

    if let Some(payload) = call.stdin {
        let mut stdin = child.stdin().ok_or(ProcessCallError::Io {
            operation: "open stdin",
            message: "child stdin unavailable".to_string(),
        })?;
        stdin
            .write_all(&payload)
            .await
            .map_err(|error| ProcessCallError::Io {
                operation: "write stdin",
                message: error.to_string(),
            })?;
        stdin
            .shutdown()
            .await
            .map_err(|error| ProcessCallError::Io {
                operation: "write stdin",
                message: error.to_string(),
            })?;
        drop(stdin);
    }

    let wait = child.wait_with_output_async(cx);
    let output = match call.timeout {
        Some(window) => {
            match timeout(crate::runtime::timer_now(cx), window, Box::pin(wait)).await {
                Ok(result) => result,
                Err(_elapsed) => {
                    return Err(ProcessCallError::TimedOut);
                }
            }
        }
        None => wait.await,
    }
    .map_err(|error| ProcessCallError::Io {
        operation: "wait",
        message: error.to_string(),
    })?;

    Ok(ProcessOutput {
        status_code: output.status.code(),
        status_display: output.status.to_string(),
        success: output.status.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}
