use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::client::{McpError, StdioMcpServerConfig};
use super::protocol::render_json;

pub(super) struct Connection {
    child: Child,
    stdin: ChildStdin,
    stdout_lines: mpsc::Receiver<String>,
    reader: Option<thread::JoinHandle<()>>,
    next_id: u64,
    killed: bool,
}

impl Connection {
    pub(super) fn spawn(config: &StdioMcpServerConfig) -> Result<Self, McpError> {
        if config.command.trim().is_empty() {
            return Err(McpError::Spawn {
                command: config.command.clone(),
                message: "command is empty".to_string(),
            });
        }

        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // A misbehaving MCP server must not deadlock us by filling stderr.
            .stderr(Stdio::null());

        let mut child = command.spawn().map_err(|error| McpError::Spawn {
            command: render_command(&config.command, &config.args),
            message: error.to_string(),
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            McpError::Protocol("spawned MCP child did not provide stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            McpError::Protocol("spawned MCP child did not provide stdout".to_string())
        })?;
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
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

        Ok(Self {
            child,
            stdin,
            stdout_lines: rx,
            reader: Some(reader),
            next_id: 1,
            killed: false,
        })
    }

    pub(super) fn child_id(&self) -> u32 {
        self.child.id()
    }

    pub(super) fn notify(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<(), McpError> {
        self.ensure_running(method)?;
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_json_line(method, request, timeout)
    }

    pub(super) fn request(
        &mut self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.ensure_running(method)?;
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_json_line(method, request, timeout)?;

        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                self.kill_child();
                return Err(McpError::Timeout {
                    method: method.to_string(),
                    timeout,
                });
            }
            let remaining = deadline.saturating_duration_since(now);
            let line = match self.stdout_lines.recv_timeout(remaining) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.kill_child();
                    return Err(McpError::Timeout {
                        method: method.to_string(),
                        timeout,
                    });
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self.process_exited_error(method));
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
                // Server notification/progress message. The minimal bridge does
                // not surface progress yet, but it must not confuse responses.
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

    fn ensure_running(&mut self, method: &str) -> Result<(), McpError> {
        if self.killed {
            return Err(McpError::ProcessExited {
                method: method.to_string(),
                status: Some("killed".to_string()),
            });
        }
        match self.child.try_wait() {
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

    fn write_json_line(
        &mut self,
        method: &str,
        request: Value,
        timeout: Duration,
    ) -> Result<(), McpError> {
        let mut bytes = serde_json::to_vec(&request).map_err(|error| McpError::Json {
            operation: "encode request",
            message: error.to_string(),
        })?;
        bytes.push(b'\n');
        if timeout.is_zero() {
            self.kill_child();
            return Err(McpError::Timeout {
                method: method.to_string(),
                timeout,
            });
        }
        self.stdin.write_all(&bytes).map_err(|error| McpError::Io {
            operation: "write request",
            message: error.to_string(),
        })?;
        self.stdin.flush().map_err(|error| McpError::Io {
            operation: "flush request",
            message: error.to_string(),
        })
    }

    fn process_exited_error(&mut self, method: &str) -> McpError {
        match self.child.try_wait() {
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

    fn kill_child(&mut self) {
        if !self.killed {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.killed = true;
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.kill_child();
    }
}

fn render_command(command: &str, args: &[String]) -> String {
    std::iter::once(command.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}
