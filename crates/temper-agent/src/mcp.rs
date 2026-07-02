//! Minimal stdio MCP/JSON-RPC client for agent-local tool bridges.
//!
//! This module intentionally implements only the low-level pieces the coding
//! agent needs for a local codebase-memory MCP server: spawn a configured
//! command over stdio, initialize, list tools, call a tool, and kill the child
//! when the last client/tool handle is dropped. It does not try to be a full MCP
//! SDK.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// MCP protocol version sent in the initialize request.
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Spawn settings for a stdio MCP server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StdioMcpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub startup_timeout: Duration,
    pub call_timeout: Duration,
}

impl StdioMcpServerConfig {
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            startup_timeout: Duration::from_secs(5),
            call_timeout: Duration::from_secs(30),
        }
    }

    pub fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }
}

/// One MCP tool descriptor returned by `tools/list`.
#[derive(Clone, Debug, PartialEq)]
pub struct McpToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Textual result of an MCP `tools/call` response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpToolCallResult {
    pub text: String,
    pub is_error: bool,
}

/// Errors from spawning or talking to a stdio MCP server.
#[derive(Debug)]
pub enum McpError {
    Spawn {
        command: String,
        message: String,
    },
    Io {
        operation: &'static str,
        message: String,
    },
    Json {
        operation: &'static str,
        message: String,
    },
    Rpc {
        method: String,
        message: String,
    },
    Timeout {
        method: String,
        timeout: Duration,
    },
    ProcessExited {
        method: String,
        status: Option<String>,
    },
    Protocol(String),
}

impl std::fmt::Display for McpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { command, message } => {
                write!(formatter, "spawn MCP command `{command}` failed: {message}")
            }
            Self::Io { operation, message } => {
                write!(formatter, "MCP {operation} failed: {message}")
            }
            Self::Json { operation, message } => {
                write!(formatter, "MCP JSON {operation} failed: {message}")
            }
            Self::Rpc { method, message } => {
                write!(formatter, "MCP request `{method}` failed: {message}")
            }
            Self::Timeout { method, timeout } => write!(
                formatter,
                "MCP request `{method}` timed out after {:.3}s",
                timeout.as_secs_f64()
            ),
            Self::ProcessExited { method, status } => match status {
                Some(status) => write!(
                    formatter,
                    "MCP process exited while waiting for `{method}` ({status})"
                ),
                None => write!(formatter, "MCP process exited while waiting for `{method}`"),
            },
            Self::Protocol(message) => write!(formatter, "MCP protocol error: {message}"),
        }
    }
}

impl std::error::Error for McpError {}

/// Cloneable handle to one stdio MCP child process.
#[derive(Clone)]
pub struct StdioMcpClient {
    inner: Arc<ClientInner>,
    call_timeout: Duration,
}

impl StdioMcpClient {
    /// Spawns the configured command, sends MCP `initialize`, and sends the
    /// standard `notifications/initialized` notification.
    pub async fn connect(config: StdioMcpServerConfig) -> Result<Self, McpError> {
        let connection = Connection::spawn(&config)?;
        let client = Self {
            inner: Arc::new(ClientInner {
                connection: Mutex::new(connection),
            }),
            call_timeout: config.call_timeout,
        };
        client.initialize(config.startup_timeout).await?;
        client
            .notify_initialized(config.startup_timeout)
            .await
            .map_err(|error| match error {
                McpError::Timeout { .. } => McpError::Timeout {
                    method: "notifications/initialized".to_string(),
                    timeout: config.startup_timeout,
                },
                other => other,
            })?;
        Ok(client)
    }

    /// Returns the configured default call timeout for this client.
    pub fn call_timeout(&self) -> Duration {
        self.call_timeout
    }

    /// Returns the child process id. Intended for focused lifecycle tests and
    /// diagnostics; callers should not attempt to manage the process directly.
    pub fn child_id(&self) -> u32 {
        self.with_connection(|connection| Ok(connection.child_id()))
            .expect("MCP connection lock is not poisoned")
    }

    /// Lists MCP tools. Pagination cursors are followed if the server returns
    /// `nextCursor`.
    pub async fn list_tools(&self, timeout: Duration) -> Result<Vec<McpToolDescriptor>, McpError> {
        let mut all_tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor
                .as_ref()
                .map(|cursor| json!({ "cursor": cursor }))
                .unwrap_or_else(|| json!({}));
            let result = self.request("tools/list", params, timeout).await?;
            let mut page = parse_tool_list(&result)?;
            all_tools.append(&mut page.tools);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(all_tools)
    }

    /// Calls one MCP tool by its server-side name.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<McpToolCallResult, McpError> {
        let arguments = if arguments.is_null() {
            json!({})
        } else {
            arguments
        };
        let result = self
            .request(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments,
                }),
                timeout,
            )
            .await?;
        Ok(parse_call_tool_result(result))
    }

    async fn initialize(&self, timeout: Duration) -> Result<(), McpError> {
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "temper-agent",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }),
                timeout,
            )
            .await?;
        if !result.is_object() {
            return Err(McpError::Protocol(
                "initialize result must be a JSON object".to_string(),
            ));
        }
        Ok(())
    }

    async fn notify_initialized(&self, timeout: Duration) -> Result<(), McpError> {
        let client = self.clone();
        skein::runtime::spawn_blocking(move || {
            client.with_connection(|connection| {
                connection.notify("notifications/initialized", json!({}), timeout)
            })
        })
        .await
    }

    async fn request(
        &self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        let client = self.clone();
        skein::runtime::spawn_blocking(move || {
            client.with_connection(|connection| connection.request(method, params, timeout))
        })
        .await
    }

    fn with_connection<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, McpError>,
    ) -> Result<T, McpError> {
        let mut connection = self
            .inner
            .connection
            .lock()
            .map_err(|_| McpError::Protocol("MCP connection lock poisoned".to_string()))?;
        f(&mut connection)
    }
}

struct ClientInner {
    connection: Mutex<Connection>,
}

struct Connection {
    child: Child,
    stdin: ChildStdin,
    stdout_lines: mpsc::Receiver<String>,
    reader: Option<thread::JoinHandle<()>>,
    next_id: u64,
    killed: bool,
}

impl Connection {
    fn spawn(config: &StdioMcpServerConfig) -> Result<Self, McpError> {
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

    fn child_id(&self) -> u32 {
        self.child.id()
    }

    fn notify(&mut self, method: &str, params: Value, timeout: Duration) -> Result<(), McpError> {
        self.ensure_running(method)?;
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_json_line(method, request, timeout)
    }

    fn request(
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

#[derive(Debug)]
struct ToolListPage {
    tools: Vec<McpToolDescriptor>,
    next_cursor: Option<String>,
}

fn parse_tool_list(result: &Value) -> Result<ToolListPage, McpError> {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            McpError::Protocol("tools/list result must contain `tools` array".to_string())
        })?
        .iter()
        .map(parse_tool_descriptor)
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = result
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(ToolListPage { tools, next_cursor })
}

fn parse_tool_descriptor(value: &Value) -> Result<McpToolDescriptor, McpError> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Protocol("MCP tool descriptor missing string `name`".to_string()))?
        .to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let input_schema = value
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(default_object_schema);
    Ok(McpToolDescriptor {
        name,
        description,
        input_schema,
    })
}

fn parse_call_tool_result(result: Value) -> McpToolCallResult {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let text = collect_result_text(&result);
    McpToolCallResult { text, is_error }
}

fn collect_result_text(result: &Value) -> String {
    let mut pieces = Vec::new();
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for block in content {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                pieces.push(text.to_string());
            } else if !block.is_null() {
                pieces.push(render_json(block));
            }
        }
    }
    if pieces.is_empty() {
        if let Some(structured) = result.get("structuredContent") {
            pieces.push(render_json(structured));
        } else {
            pieces.push(render_json(result));
        }
    }
    pieces.join("\n")
}

fn default_object_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn render_command(command: &str, args: &[String]) -> String {
    std::iter::once(command.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unrenderable JSON>".to_string())
}

#[cfg(test)]
mod tests;
