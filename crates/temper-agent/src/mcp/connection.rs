use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use temper_agent_core::{ProcessContainment, configure_descendant_command};

use super::client::{McpError, StdioMcpServerConfig};
use super::protocol::render_json;

/// Process ownership deliberately lives outside the protocol request mutex.
/// A dropped request can therefore close/kill/reap the server and wake the
/// mutex holder without first acquiring that mutex.
pub(super) struct ProcessControl {
    child_id: u32,
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    reader: Mutex<Option<thread::JoinHandle<()>>>,
    containment: ProcessContainment,
    cancelled: AtomicBool,
}

impl ProcessControl {
    fn new(
        child: Child,
        stdin: ChildStdin,
        reader: thread::JoinHandle<()>,
        containment: ProcessContainment,
    ) -> Self {
        Self {
            child_id: child.id(),
            child: Mutex::new(child),
            stdin: Mutex::new(Some(stdin)),
            reader: Mutex::new(Some(reader)),
            containment,
            cancelled: AtomicBool::new(false),
        }
    }

    pub(super) fn child_id(&self) -> u32 {
        self.child_id
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn write_json(&self, method: &str, request: Value, timeout: Duration) -> Result<(), McpError> {
        if timeout.is_zero() {
            self.cancel_and_join();
            return Err(McpError::Timeout {
                method: method.to_string(),
                timeout,
            });
        }
        if self.is_cancelled() {
            return Err(McpError::Cancelled {
                method: method.to_string(),
            });
        }
        let mut bytes = serde_json::to_vec(&request).map_err(|error| McpError::Json {
            operation: "encode request",
            message: error.to_string(),
        })?;
        bytes.push(b'\n');
        let mut stdin = self
            .stdin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stdin = stdin.as_mut().ok_or_else(|| McpError::Cancelled {
            method: method.to_string(),
        })?;
        stdin.write_all(&bytes).map_err(|error| McpError::Io {
            operation: "write request",
            message: error.to_string(),
        })?;
        stdin.flush().map_err(|error| McpError::Io {
            operation: "flush request",
            message: error.to_string(),
        })
    }

    fn ensure_running(&self, method: &str) -> Result<(), McpError> {
        if self.is_cancelled() {
            return Err(McpError::Cancelled {
                method: method.to_string(),
            });
        }
        let mut child = self
            .child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match child.try_wait() {
            Ok(Some(status)) => Err(McpError::ProcessExited {
                method: method.to_string(),
                status: Some(status.to_string()),
            }),
            Ok(None) => Ok(()),
            Err(error) => Err(McpError::Io {
                operation: "check child status",
                message: error.to_string(),
            }),
        }
    }

    fn process_exited_error(&self, method: &str) -> McpError {
        if self.is_cancelled() {
            return McpError::Cancelled {
                method: method.to_string(),
            };
        }
        let mut child = self
            .child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match child.try_wait() {
            Ok(Some(status)) => McpError::ProcessExited {
                method: method.to_string(),
                status: Some(status.to_string()),
            },
            Ok(None) => McpError::ProcessExited {
                method: method.to_string(),
                status: None,
            },
            Err(error) => McpError::Io {
                operation: "check child status",
                message: error.to_string(),
            },
        }
    }

    /// Idempotently closes stdin, kills the complete server subtree, reaps the
    /// direct child, and joins the stdout reader. Kill happens before taking the
    /// stdin lock so it can interrupt a writer blocked on a full pipe.
    pub(super) fn cancel_and_join(&self) {
        let first = !self.cancelled.swap(true, Ordering::AcqRel);
        if first {
            {
                let mut child = self
                    .child
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let _ = self.containment.hard_kill(&mut child);
                let _ = child.kill();
            }
            self.stdin
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            let mut child = self
                .child
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let _ = child.wait();
        }
        if let Some(reader) = self
            .reader
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = reader.join();
        }
    }
}

pub(super) struct Connection {
    control: Arc<ProcessControl>,
    stdout_lines: mpsc::Receiver<String>,
    next_id: u64,
}

impl Connection {
    pub(super) fn spawn(config: &StdioMcpServerConfig) -> Result<Self, McpError> {
        if config.command.trim().is_empty() {
            return Err(McpError::Spawn {
                command: config.command.clone(),
                message: "command is empty".to_string(),
            });
        }
        #[cfg(unix)]
        if !command_resolves(&config.command) {
            return Err(McpError::Spawn {
                command: render_command(&config.command, &config.args),
                message: "command was not found on PATH".to_string(),
            });
        }

        let mut command = server_command(config);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // A misbehaving MCP server must not deadlock us by filling stderr.
            .stderr(Stdio::null());
        configure_descendant_command(&mut command);

        let mut child = command.spawn().map_err(|error| McpError::Spawn {
            command: render_command(&config.command, &config.args),
            message: error.to_string(),
        })?;
        let containment = ProcessContainment::attach(&child).map_err(|error| {
            let _ = child.kill();
            let _ = child.wait();
            McpError::Spawn {
                command: render_command(&config.command, &config.args),
                message: format!("attach process containment: {error}"),
            }
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            McpError::Protocol("spawned MCP child did not provide stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            McpError::Protocol("spawned MCP child did not provide stdout".to_string())
        })?;
        let (tx, rx) = mpsc::channel();
        let reader = thread::Builder::new()
            .name(format!("mcp-stdout-{}", child.id()))
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(line) => {
                            if tx.send(line).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| {
                let _ = containment.hard_kill(&mut child);
                let _ = child.wait();
                McpError::Spawn {
                    command: render_command(&config.command, &config.args),
                    message: format!("start stdout reader: {error}"),
                }
            })?;
        let control = Arc::new(ProcessControl::new(child, stdin, reader, containment));

        Ok(Self {
            control,
            stdout_lines: rx,
            next_id: 1,
        })
    }

    pub(super) fn control(&self) -> Arc<ProcessControl> {
        Arc::clone(&self.control)
    }

    pub(super) fn notify(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<(), McpError> {
        self.control.ensure_running(method)?;
        self.control.write_json(
            method,
            json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }),
            timeout,
        )
    }

    pub(super) fn request(
        &mut self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.control.ensure_running(method)?;
        let id = self.next_id;
        self.next_id += 1;
        self.control.write_json(
            method,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            }),
            timeout,
        )?;

        let deadline = Instant::now() + timeout;
        loop {
            if self.control.is_cancelled() {
                return Err(McpError::Cancelled {
                    method: method.to_string(),
                });
            }
            let now = Instant::now();
            if now >= deadline {
                self.control.cancel_and_join();
                return Err(McpError::Timeout {
                    method: method.to_string(),
                    timeout,
                });
            }
            let remaining = deadline.saturating_duration_since(now);
            let line = match self.stdout_lines.recv_timeout(remaining) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.control.cancel_and_join();
                    return Err(McpError::Timeout {
                        method: method.to_string(),
                        timeout,
                    });
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self.control.process_exited_error(method));
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let response: Value = serde_json::from_str(&line).map_err(|error| McpError::Json {
                operation: "decode response",
                message: error.to_string(),
            })?;
            let Some(response_id) = response.get("id") else {
                continue;
            };
            if response_id != &Value::from(id) {
                return Err(McpError::Protocol(format!(
                    "response id {response_id} did not match request id {id}"
                )));
            }
            if let Some(error) = response.get("error") {
                return Err(McpError::Rpc {
                    method: method.to_string(),
                    message: render_json(error),
                });
            }
            return Ok(response.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

#[cfg(unix)]
fn command_resolves(command: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg("command -v -- \"$1\" >/dev/null 2>&1")
        .arg("temper-mcp-command-check")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn server_command(config: &StdioMcpServerConfig) -> Command {
    #[cfg(unix)]
    {
        // Keep a small group leader alive so Linux parent-death SIGTERM can be
        // relayed as SIGKILL to the complete MCP server subtree.
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("trap 'trap - TERM; kill -KILL -- -$$' TERM; \"$@\"")
            .arg("temper-mcp-server")
            .arg(&config.command)
            .args(&config.args);
        command
    }
    #[cfg(not(unix))]
    {
        let mut command = Command::new(&config.command);
        command.args(&config.args);
        command
    }
}

fn render_command(command: &str, args: &[String]) -> String {
    std::iter::once(command.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}
