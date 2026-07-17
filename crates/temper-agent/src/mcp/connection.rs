use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use temper_agent_core::{
    AgentContainmentContext, CleanupTrigger, ContainedProcess, ContainmentCommand, ContainmentScope,
};

use super::client::{McpError, StdioMcpServerConfig};
use super::protocol::render_json;

#[cfg(test)]
static ACTIVE_OUTPUT_READERS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static JOINED_OUTPUT_READERS: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

#[cfg(test)]
pub(super) fn active_output_readers() -> usize {
    ACTIVE_OUTPUT_READERS.load(Ordering::Acquire)
}

#[cfg(test)]
pub(super) fn output_reader_joined(config: &StdioMcpServerConfig) -> bool {
    JOINED_OUTPUT_READERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&render_command(&config.command, &config.args))
}

#[cfg(test)]
struct ActiveReaderGuard(String);

#[cfg(test)]
impl ActiveReaderGuard {
    fn enter(key: String) -> Self {
        ACTIVE_OUTPUT_READERS.fetch_add(1, Ordering::AcqRel);
        Self(key)
    }
}

#[cfg(test)]
impl Drop for ActiveReaderGuard {
    fn drop(&mut self) {
        ACTIVE_OUTPUT_READERS.fetch_sub(1, Ordering::AcqRel);
        JOINED_OUTPUT_READERS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(self.0.clone());
    }
}

/// Process ownership deliberately lives outside the protocol request mutex.
/// A dropped request can therefore terminate/prove-empty the complete server
/// and wake a blocked writer without first acquiring that mutex.
pub(super) struct ProcessControl {
    child_id: u32,
    process: ContainedProcess,
    stdin: Mutex<Option<std::process::ChildStdin>>,
    reader: Mutex<Option<thread::JoinHandle<()>>>,
    cancelled: AtomicBool,
}

impl ProcessControl {
    fn new(
        process: ContainedProcess,
        stdin: std::process::ChildStdin,
        reader: thread::JoinHandle<()>,
    ) -> Self {
        Self {
            child_id: process.id(),
            process,
            stdin: Mutex::new(Some(stdin)),
            reader: Mutex::new(Some(reader)),
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
            self.cancel_and_join(CleanupTrigger::Timeout);
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
        let write = {
            let mut stdin = self
                .stdin
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(stdin) = stdin.as_mut() else {
                return Err(McpError::Cancelled {
                    method: method.to_string(),
                });
            };
            stdin.write_all(&bytes).and_then(|()| stdin.flush())
        };
        if let Err(error) = write {
            let error = McpError::Io {
                operation: "write request",
                message: error.to_string(),
            };
            self.cancel_and_join(CleanupTrigger::Cancellation);
            return Err(error);
        }
        Ok(())
    }

    fn ensure_running(&self, method: &str) -> Result<(), McpError> {
        if self.is_cancelled() {
            return Err(McpError::Cancelled {
                method: method.to_string(),
            });
        }
        match self.process.try_wait_root() {
            Ok(Some(status)) => {
                let error = McpError::ProcessExited {
                    method: method.to_string(),
                    status: Some(status.to_string()),
                };
                self.cancel_and_join(CleanupTrigger::NormalRootExit);
                Err(error)
            }
            Ok(None) => Ok(()),
            Err(error) => {
                let error = McpError::Io {
                    operation: "check child status",
                    message: error.to_string(),
                };
                self.cancel_and_join(CleanupTrigger::Cancellation);
                Err(error)
            }
        }
    }

    fn process_exited_error_and_join(&self, method: &str) -> McpError {
        if self.is_cancelled() {
            self.cancel_and_join(CleanupTrigger::Cancellation);
            return McpError::Cancelled {
                method: method.to_string(),
            };
        }
        let error = match self.process.try_wait_root() {
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
        };
        self.cancel_and_join(CleanupTrigger::NormalRootExit);
        error
    }

    fn protocol_failure(&self, error: McpError) -> McpError {
        self.cancel_and_join(CleanupTrigger::Cancellation);
        error
    }

    /// Idempotently interrupts blocked writers by terminating the process
    /// without the stdin lock, waits for recursive emptiness/direct-child reap,
    /// closes stdin, and joins the stdout reader exactly once.
    pub(super) fn cancel_and_join(&self, trigger: CleanupTrigger) {
        self.cancelled.store(true, Ordering::Release);
        let _report = self.process.cleanup(trigger);
        self.stdin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
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
    pub(super) fn spawn(
        config: &StdioMcpServerConfig,
        containment: &AgentContainmentContext,
    ) -> Result<Self, McpError> {
        if config.command.trim().is_empty() {
            return Err(McpError::Spawn {
                command: config.command.clone(),
                message: "command is empty".to_string(),
            });
        }

        let spec = containment.containment_spec(&config.command, ContainmentScope::McpServer);
        let prepared = containment
            .factory()
            .prepare(spec)
            .map_err(|error| McpError::Spawn {
                command: render_command(&config.command, &config.args),
                message: format!("prepare process containment: {error}"),
            })?;
        let mut command = server_command(config);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // A misbehaving MCP server must not deadlock us by filling stderr.
            .stderr(Stdio::null());
        let process = prepared.spawn(command).map_err(|error| McpError::Spawn {
            command: render_command(&config.command, &config.args),
            message: error.to_string(),
        })?;
        let stdin = match process.take_stdin() {
            Ok(Some(stdin)) => stdin,
            Ok(None) => {
                let _report = process.cleanup(CleanupTrigger::Shutdown);
                return Err(McpError::Protocol(
                    "spawned MCP child did not provide stdin".to_string(),
                ));
            }
            Err(error) => {
                let _report = process.cleanup(CleanupTrigger::Shutdown);
                return Err(McpError::Spawn {
                    command: render_command(&config.command, &config.args),
                    message: format!("take child stdin: {error}"),
                });
            }
        };
        let stdout = match process.take_stdout() {
            Ok(Some(stdout)) => stdout,
            Ok(None) => {
                drop(stdin);
                let _report = process.cleanup(CleanupTrigger::Shutdown);
                return Err(McpError::Protocol(
                    "spawned MCP child did not provide stdout".to_string(),
                ));
            }
            Err(error) => {
                drop(stdin);
                let _report = process.cleanup(CleanupTrigger::Shutdown);
                return Err(McpError::Spawn {
                    command: render_command(&config.command, &config.args),
                    message: format!("take child stdout: {error}"),
                });
            }
        };
        let (tx, rx) = mpsc::channel();
        #[cfg(test)]
        let reader_key = render_command(&config.command, &config.args);
        let reader = thread::Builder::new()
            .name(format!("mcp-stdout-{}", process.id()))
            .spawn(move || {
                #[cfg(test)]
                let _active_reader = ActiveReaderGuard::enter(reader_key);
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
            });
        let reader = match reader {
            Ok(reader) => reader,
            Err(error) => {
                drop(stdin);
                let _report = process.cleanup(CleanupTrigger::Shutdown);
                return Err(McpError::Spawn {
                    command: render_command(&config.command, &config.args),
                    message: format!("start stdout reader: {error}"),
                });
            }
        };
        let control = Arc::new(ProcessControl::new(process, stdin, reader));

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
        )?;
        self.control.ensure_running(method)
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
                self.control.cancel_and_join(CleanupTrigger::Cancellation);
                return Err(McpError::Cancelled {
                    method: method.to_string(),
                });
            }
            let now = Instant::now();
            if now >= deadline {
                self.control.cancel_and_join(CleanupTrigger::Timeout);
                return Err(McpError::Timeout {
                    method: method.to_string(),
                    timeout,
                });
            }
            let remaining = deadline.saturating_duration_since(now);
            let line = match self.stdout_lines.recv_timeout(remaining) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.control.cancel_and_join(CleanupTrigger::Timeout);
                    return Err(McpError::Timeout {
                        method: method.to_string(),
                        timeout,
                    });
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self.control.process_exited_error_and_join(method));
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let response: Value = match serde_json::from_str(&line) {
                Ok(response) => response,
                Err(error) => {
                    return Err(self.control.protocol_failure(McpError::Json {
                        operation: "decode response",
                        message: error.to_string(),
                    }));
                }
            };
            let Some(response_id) = response.get("id") else {
                continue;
            };
            if response_id != &Value::from(id) {
                return Err(self.control.protocol_failure(McpError::Protocol(format!(
                    "response id {response_id} did not match request id {id}"
                ))));
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

fn server_command(config: &StdioMcpServerConfig) -> ContainmentCommand {
    let mut command = ContainmentCommand::new(config.command.as_str());
    command.args(&config.args);
    command
}

fn render_command(command: &str, args: &[String]) -> String {
    std::iter::once(command.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}
