//! Stdio MCP client and child-process lifecycle management.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::connection::Connection;
use super::protocol::{
    McpToolCallResult, McpToolDescriptor, parse_call_tool_result, parse_tool_list,
};

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

    /// Spawns the configured command and initializes it from synchronous code.
    ///
    /// This is used only for host-controlled background bootstrap work where we
    /// deliberately do not want to hold up the agent's main skein task. Normal
    /// MCP use should prefer [`Self::connect`].
    pub fn connect_blocking(config: StdioMcpServerConfig) -> Result<Self, McpError> {
        let connection = Connection::spawn(&config)?;
        let client = Self {
            inner: Arc::new(ClientInner {
                connection: Mutex::new(connection),
            }),
            call_timeout: config.call_timeout,
        };
        client.initialize_blocking(config.startup_timeout)?;
        client
            .notify_initialized_blocking(config.startup_timeout)
            .map_err(|error| match error {
                McpError::Timeout { .. } => McpError::Timeout {
                    method: "notifications/initialized".to_string(),
                    timeout: config.startup_timeout,
                },
                other => other,
            })?;
        Ok(client)
    }

    /// Calls one MCP tool by its server-side name from synchronous code.
    pub fn call_tool_blocking(
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
        let result = self.request_blocking(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
            }),
            timeout,
        )?;
        Ok(parse_call_tool_result(result))
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

    fn initialize_blocking(&self, timeout: Duration) -> Result<(), McpError> {
        let result = self.request_blocking(
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
        )?;
        if !result.is_object() {
            return Err(McpError::Protocol(
                "initialize result must be a JSON object".to_string(),
            ));
        }
        Ok(())
    }

    fn notify_initialized_blocking(&self, timeout: Duration) -> Result<(), McpError> {
        self.with_connection(|connection| {
            connection.notify("notifications/initialized", json!({}), timeout)
        })
    }

    fn request_blocking(
        &self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.with_connection(|connection| connection.request(method, params, timeout))
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
