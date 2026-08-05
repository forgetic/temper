//! Stdio MCP client and child-process lifecycle management.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

use temper_agent_core::{AgentContainmentContext, CleanupTrigger};

use super::connection::{Connection, ProcessControl};
use super::protocol::{
    McpToolCallResult, McpToolDescriptor, parse_call_tool_result, parse_tool_list,
};

/// MCP protocol version sent in the initialize request.
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

const MAX_METADATA_STRING_BYTES: usize = 128;
const MAX_CAPABILITY_NAMES: usize = 32;

/// Bounded provider metadata retained from the MCP `initialize` result.
///
/// Only identity and top-level capability names are retained. This gives
/// integrations enough information to enforce a provider contract without
/// keeping an attacker-controlled initialize document alive for the process
/// lifetime.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct McpServerMetadata {
    pub protocol_version: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub capabilities: BTreeSet<String>,
}

impl McpServerMetadata {
    pub fn advertises_capability(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }
}

/// Spawn settings for a stdio MCP server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StdioMcpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub startup_timeout: Duration,
    pub call_timeout: Duration,
    containment_identity: String,
}

impl StdioMcpServerConfig {
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            startup_timeout: Duration::from_secs(5),
            call_timeout: Duration::from_secs(30),
            containment_identity: "mcp-server".to_string(),
        }
    }

    /// Sets the content-free operator identity for this server containment.
    /// The executable and argument vector are deliberately not used as event
    /// identity because they may contain workspace paths or credentials.
    pub(crate) fn with_containment_identity(mut self, identity: impl Into<String>) -> Self {
        self.containment_identity = identity.into();
        self
    }

    pub(super) fn containment_identity(&self) -> &str {
        &self.containment_identity
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
    Cancelled {
        method: String,
    },
    ProcessExited {
        method: String,
        status: Option<String>,
    },
    ProtocolOverflow {
        direction: &'static str,
        resource: &'static str,
        limit: usize,
        observed: usize,
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
            Self::Cancelled { method } => write!(formatter, "MCP request `{method}` was cancelled"),
            Self::ProcessExited { method, status } => match status {
                Some(status) => write!(
                    formatter,
                    "MCP process exited while waiting for `{method}` ({status})"
                ),
                None => write!(formatter, "MCP process exited while waiting for `{method}`"),
            },
            Self::ProtocolOverflow {
                direction,
                resource,
                limit,
                observed,
            } => write!(
                formatter,
                "MCP protocol overflow: {direction} {resource} exceeded limit {limit} (observed at least {observed})"
            ),
            Self::Protocol(message) => write!(formatter, "MCP protocol error: {message}"),
        }
    }
}

impl std::error::Error for McpError {}

/// Cloneable cancellation authority independent of the serialized MCP request
/// mutex. Cancellation closes stdin, kills/reaps the server containment group,
/// disconnects a waiting response receiver, and joins the reader thread.
#[derive(Clone)]
pub struct McpCancellationHandle {
    control: Arc<ProcessControl>,
}

impl McpCancellationHandle {
    pub fn cancel_and_join(&self) {
        self.control.cancel_and_join(CleanupTrigger::Cancellation);
    }

    /// Requests cleanup on the process's dedicated owner. This path never
    /// waits for the serialized request mutex or recursive-empty proof.
    pub fn request_cancel(&self) {
        self.control.request_cleanup(CleanupTrigger::Cancellation);
    }
}

impl std::fmt::Debug for McpCancellationHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpCancellationHandle")
            .field("child_id", &self.control.child_id())
            .finish()
    }
}

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
        Self::connect_with_containment(config, default_containment_context()).await
    }

    /// Connects with the concrete containment context owned by the surrounding
    /// agent session.
    pub async fn connect_with_containment(
        config: StdioMcpServerConfig,
        containment: AgentContainmentContext,
    ) -> Result<Self, McpError> {
        let client = Self::spawn(&config, &containment)?;
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
    pub fn connect_blocking(config: StdioMcpServerConfig) -> Result<Self, McpError> {
        Self::connect_blocking_with_containment(config, default_containment_context())
    }

    pub fn connect_blocking_with_containment(
        config: StdioMcpServerConfig,
        containment: AgentContainmentContext,
    ) -> Result<Self, McpError> {
        let client = Self::spawn(&config, &containment)?;
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

    fn spawn(
        config: &StdioMcpServerConfig,
        containment: &AgentContainmentContext,
    ) -> Result<Self, McpError> {
        let connection = Connection::spawn(config, containment)?;
        let control = connection.control();
        Ok(Self {
            inner: Arc::new(ClientInner {
                connection: Mutex::new(connection),
                control,
                server_metadata: Mutex::new(None),
            }),
            call_timeout: config.call_timeout,
        })
    }

    /// Calls one MCP tool by its server-side name from synchronous code.
    pub fn call_tool_blocking(
        &self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<McpToolCallResult, McpError> {
        let arguments = normalize_arguments(arguments);
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

    pub fn call_timeout(&self) -> Duration {
        self.call_timeout
    }

    /// Returns the bounded metadata captured during MCP initialization.
    pub fn server_metadata(&self) -> Option<McpServerMetadata> {
        self.inner
            .server_metadata
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn child_id(&self) -> u32 {
        self.inner.control.child_id()
    }

    pub fn cancellation_handle(&self) -> McpCancellationHandle {
        McpCancellationHandle {
            control: Arc::clone(&self.inner.control),
        }
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

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<McpToolCallResult, McpError> {
        let result = self
            .request(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": normalize_arguments(arguments),
                }),
                timeout,
            )
            .await?;
        Ok(parse_call_tool_result(result))
    }

    fn initialize_blocking(&self, timeout: Duration) -> Result<(), McpError> {
        let result = self.request_blocking("initialize", initialize_params(), timeout)?;
        let metadata = parse_initialize_metadata(result)?;
        self.store_server_metadata(metadata)
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
            .request("initialize", initialize_params(), timeout)
            .await?;
        let metadata = parse_initialize_metadata(result)?;
        self.store_server_metadata(metadata)
    }

    fn store_server_metadata(&self, metadata: McpServerMetadata) -> Result<(), McpError> {
        *self
            .inner
            .server_metadata
            .lock()
            .map_err(|_| McpError::Protocol("MCP metadata lock poisoned".to_string()))? =
            Some(metadata);
        Ok(())
    }

    async fn notify_initialized(&self, timeout: Duration) -> Result<(), McpError> {
        let client = self.clone();
        let control = self.cancellation_handle();
        BlockingCall::spawn(control, "mcp-notify", move || {
            client.with_connection(|connection| {
                connection.notify("notifications/initialized", json!({}), timeout)
            })
        })?
        .await
    }

    async fn request(
        &self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        let client = self.clone();
        let control = self.cancellation_handle();
        BlockingCall::spawn(control, "mcp-request", move || {
            client.with_connection(|connection| connection.request(method, params, timeout))
        })?
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

fn default_containment_context() -> AgentContainmentContext {
    #[cfg(test)]
    {
        crate::containment_tests::containment_context()
    }
    #[cfg(not(test))]
    AgentContainmentContext::production(None)
}

fn normalize_arguments(arguments: Value) -> Value {
    if arguments.is_null() {
        json!({})
    } else {
        arguments
    }
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {
            "name": "temper-agent",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

pub(super) fn parse_initialize_metadata(result: Value) -> Result<McpServerMetadata, McpError> {
    let object = result
        .as_object()
        .ok_or_else(|| McpError::Protocol("initialize result must be a JSON object".to_string()))?;
    let server_info = object.get("serverInfo").and_then(Value::as_object);
    let capabilities = object
        .get("capabilities")
        .and_then(Value::as_object)
        .map(|capabilities| {
            capabilities
                .keys()
                .take(MAX_CAPABILITY_NAMES)
                .map(|name| bounded_metadata_string(name))
                .collect()
        })
        .unwrap_or_default();
    Ok(McpServerMetadata {
        protocol_version: bounded_optional_string(object.get("protocolVersion")),
        name: server_info.and_then(|info| bounded_optional_string(info.get("name"))),
        version: server_info.and_then(|info| bounded_optional_string(info.get("version"))),
        capabilities,
    })
}

fn bounded_optional_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(bounded_metadata_string)
}

fn bounded_metadata_string(value: &str) -> String {
    let mut end = value.len().min(MAX_METADATA_STRING_BYTES);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

struct ClientInner {
    connection: Mutex<Connection>,
    control: Arc<ProcessControl>,
    server_metadata: Mutex<Option<McpServerMetadata>>,
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        self.control.request_cleanup(CleanupTrigger::OwnerDrop);
    }
}

struct BlockingCallState<T> {
    result: Option<Result<T, McpError>>,
    waker: Option<Waker>,
}

/// One joined blocking MCP operation. Unlike `spawn_blocking`, dropping this
/// future cannot detach a mutex waiter or socket/pipe read from the agent run.
struct BlockingCall<T> {
    state: Arc<Mutex<BlockingCallState<T>>>,
    cancellation: McpCancellationHandle,
    thread: Option<JoinHandle<()>>,
    completed: bool,
}

impl<T: Send + 'static> BlockingCall<T> {
    fn spawn(
        cancellation: McpCancellationHandle,
        name: &'static str,
        operation: impl FnOnce() -> Result<T, McpError> + Send + 'static,
    ) -> Result<Self, McpError> {
        let state = Arc::new(Mutex::new(BlockingCallState {
            result: None,
            waker: None,
        }));
        let thread_state = Arc::clone(&state);
        let thread = thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let result = operation();
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
            .map_err(|error| McpError::Io {
                operation: "start request thread",
                message: error.to_string(),
            })?;
        Ok(Self {
            state,
            cancellation,
            thread: Some(thread),
            completed: false,
        })
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl<T: Send + 'static> Future for BlockingCall<T> {
    type Output = Result<T, McpError>;

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
                self.completed = true;
                self.join();
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }
}

impl<T> Drop for BlockingCall<T> {
    fn drop(&mut self) {
        if !self.completed {
            self.cancellation.request_cancel();
        }
        // The request thread exits after the independent cleanup owner closes
        // the process/pipe. Detach rather than joining from standalone's loop.
        let _ = self.thread.take();
    }
}
