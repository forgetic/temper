//! Codebase-memory MCP allowlist and read-only tongs tool wrappers.
//!
//! The public entry point is [`build_codebase_memory_toolset`]: pass the parsed
//! worker→agent [`temper_protocol_agent::AgentToolConfig`] and the current role,
//! and it returns a set of safe, prefixed, read-only tools plus metadata for
//! prompt generation.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use temper_protocol_agent::{AgentToolConfig, CodebaseMemoryMode, CodebaseMemoryToolConfig};
use tongs::error::{Error, Result};
use tongs::model::{ContentBlock, TextContent};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolRegistry, ToolUpdate};

use crate::mcp::{McpError, McpToolDescriptor, StdioMcpClient, StdioMcpServerConfig};

/// Maximum UTF-8 bytes returned to the model from one MCP tool call.
pub(crate) const MAX_CODEBASE_MEMORY_OUTPUT_BYTES: usize = 16 * 1024;

/// MCP tools considered safe for the initial bridge.
const ALLOWLIST: &[AllowedCodebaseMemoryTool] = &[
    AllowedCodebaseMemoryTool::new("get_architecture", "codebase_memory_get_architecture"),
    AllowedCodebaseMemoryTool::new("search_graph", "codebase_memory_search_graph"),
    AllowedCodebaseMemoryTool::new("trace_path", "codebase_memory_trace_path"),
    AllowedCodebaseMemoryTool::new("get_code_snippet", "codebase_memory_get_code_snippet"),
    AllowedCodebaseMemoryTool::new("get_graph_schema", "codebase_memory_get_graph_schema"),
    AllowedCodebaseMemoryTool::new("search_code", "codebase_memory_search_code"),
    AllowedCodebaseMemoryTool::new("list_projects", "codebase_memory_list_projects"),
    AllowedCodebaseMemoryTool::new("index_status", "codebase_memory_index_status"),
    AllowedCodebaseMemoryTool::new("detect_changes", "codebase_memory_detect_changes"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AllowedCodebaseMemoryTool {
    mcp_name: &'static str,
    public_name: &'static str,
}

impl AllowedCodebaseMemoryTool {
    const fn new(mcp_name: &'static str, public_name: &'static str) -> Self {
        Self {
            mcp_name,
            public_name,
        }
    }
}

/// The result of building the optional codebase-memory toolset.
pub struct CodebaseMemoryToolset {
    status: CodebaseMemoryToolsetStatus,
    registered_tool_names: Vec<String>,
    registered_tool_metadata: Vec<CodebaseMemoryToolMetadata>,
    tools: Vec<Box<dyn Tool>>,
}

/// Agent-facing metadata for one registered safe codebase-memory tool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodebaseMemoryToolMetadata {
    pub name: String,
    pub description: String,
}

impl CodebaseMemoryToolset {
    fn disabled(status: CodebaseMemoryToolsetStatus) -> Self {
        Self {
            status,
            registered_tool_names: Vec::new(),
            registered_tool_metadata: Vec::new(),
            tools: Vec::new(),
        }
    }

    fn started(
        tools: Vec<Box<dyn Tool>>,
        registered_tool_metadata: Vec<CodebaseMemoryToolMetadata>,
    ) -> Self {
        let registered_tool_names = registered_tool_metadata
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        Self {
            status: CodebaseMemoryToolsetStatus::Started,
            registered_tool_names,
            registered_tool_metadata,
            tools,
        }
    }

    /// Status explaining whether tools were registered or why they were not.
    pub fn status(&self) -> &CodebaseMemoryToolsetStatus {
        &self.status
    }

    /// Stable agent-facing tool names registered from the MCP server.
    pub fn registered_tool_names(&self) -> &[String] {
        &self.registered_tool_names
    }

    /// Agent-facing tool names and descriptions registered from the MCP server.
    pub fn registered_tool_metadata(&self) -> &[CodebaseMemoryToolMetadata] {
        &self.registered_tool_metadata
    }

    /// Consumes the toolset and returns the wrapped tongs tools.
    pub fn into_tools(self) -> Vec<Box<dyn Tool>> {
        self.tools
    }

    /// Appends this toolset to an existing [`ToolRegistry`].
    pub fn append_to_registry(self, registry: &mut ToolRegistry) {
        for tool in self.tools {
            registry.push(tool);
        }
    }
}

/// Toolset build status. `AutoUnavailable` is intentionally a success status:
/// `auto` mode is best-effort and should not fail an agent run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodebaseMemoryToolsetStatus {
    NotConfigured,
    NotEnabledForRole { role: String },
    Started,
    AutoUnavailable { reason: String },
}

/// Hard-fail error returned only for `required` mode startup/list failures.
#[derive(Debug)]
pub struct CodebaseMemoryToolsetError {
    message: String,
}

impl CodebaseMemoryToolsetError {
    fn required_startup(error: McpError) -> Self {
        Self {
            message: format!("required codebase-memory MCP startup failed: {error}"),
        }
    }
}

impl std::fmt::Display for CodebaseMemoryToolsetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CodebaseMemoryToolsetError {}

/// Builds a codebase-memory MCP toolset from the parsed agent tool config.
///
/// Error behavior is mode-dependent:
///
/// - absent config or a role mismatch returns an empty, disabled toolset;
/// - `mode = auto` returns an empty `AutoUnavailable` toolset on MCP
///   spawn/initialize/list failure;
/// - `mode = required` returns [`CodebaseMemoryToolsetError`] for those same
///   startup failures.
///
/// Index/bootstrap behavior is deliberately not implemented in this slice; the
/// existing `index_timeout_secs` protocol field is used as the per-call timeout
/// until the next config slice adds a dedicated call timeout.
pub async fn build_codebase_memory_toolset(
    config: Option<&AgentToolConfig>,
    role: &str,
) -> std::result::Result<CodebaseMemoryToolset, CodebaseMemoryToolsetError> {
    let Some(codebase_memory) = config.and_then(|config| config.codebase_memory.as_ref()) else {
        return Ok(CodebaseMemoryToolset::disabled(
            CodebaseMemoryToolsetStatus::NotConfigured,
        ));
    };
    if !codebase_memory.applies_to_role(role) {
        return Ok(CodebaseMemoryToolset::disabled(
            CodebaseMemoryToolsetStatus::NotEnabledForRole {
                role: role.to_string(),
            },
        ));
    }

    match start_required_toolset(codebase_memory).await {
        Ok(toolset) => Ok(toolset),
        Err(error) if codebase_memory.mode == CodebaseMemoryMode::Auto => Ok(
            CodebaseMemoryToolset::disabled(CodebaseMemoryToolsetStatus::AutoUnavailable {
                reason: error.to_string(),
            }),
        ),
        Err(error) => Err(CodebaseMemoryToolsetError::required_startup(error)),
    }
}

async fn start_required_toolset(
    config: &CodebaseMemoryToolConfig,
) -> std::result::Result<CodebaseMemoryToolset, McpError> {
    let startup_timeout = Duration::from_secs(config.startup_timeout_secs);
    let call_timeout = Duration::from_secs(config.index_timeout_secs);
    let mcp_config = StdioMcpServerConfig::new(config.command.clone(), config.args.clone())
        .with_startup_timeout(startup_timeout)
        .with_call_timeout(call_timeout);
    let client = StdioMcpClient::connect(mcp_config).await?;
    let advertised = client.list_tools(startup_timeout).await?;
    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    let mut registered_tool_metadata = Vec::new();

    for descriptor in advertised {
        let Some(allowed) = allowed_tool(&descriptor.name) else {
            continue;
        };
        let public_name = allowed.public_name.to_string();
        let description = description_for(*allowed, &descriptor.description);
        registered_tool_metadata.push(CodebaseMemoryToolMetadata {
            name: public_name.clone(),
            description: description.clone(),
        });
        tools.push(Box::new(CodebaseMemoryTool::new(
            client.clone(),
            descriptor,
            *allowed,
            public_name,
            description,
            call_timeout,
        )));
    }

    Ok(CodebaseMemoryToolset::started(
        tools,
        registered_tool_metadata,
    ))
}

fn allowed_tool(name: &str) -> Option<&'static AllowedCodebaseMemoryTool> {
    ALLOWLIST.iter().find(|tool| tool.mcp_name == name)
}

struct CodebaseMemoryTool {
    client: StdioMcpClient,
    mcp_name: String,
    public_name: String,
    description: String,
    parameters: Value,
    call_timeout: Duration,
}

impl CodebaseMemoryTool {
    fn new(
        client: StdioMcpClient,
        descriptor: McpToolDescriptor,
        allowed: AllowedCodebaseMemoryTool,
        public_name: String,
        description: String,
        call_timeout: Duration,
    ) -> Self {
        debug_assert_eq!(public_name, allowed.public_name);
        Self {
            client,
            mcp_name: descriptor.name,
            public_name,
            description,
            parameters: descriptor.input_schema,
            call_timeout,
        }
    }
}

#[async_trait]
impl Tool for CodebaseMemoryTool {
    fn name(&self) -> &str {
        &self.public_name
    }

    fn label(&self) -> &str {
        &self.public_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    fn effects(&self) -> ToolEffects {
        ToolEffects::read()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        input: Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> Result<ToolOutput> {
        let result = self
            .client
            .call_tool(&self.mcp_name, input, self.call_timeout)
            .await
            .map_err(|error| Error::tool(self.public_name.clone(), error))?;
        let bounded = bound_text(&result.text, MAX_CODEBASE_MEMORY_OUTPUT_BYTES);
        Ok(ToolOutput {
            content: vec![ContentBlock::Text(TextContent {
                text: bounded.text,
                text_signature: None,
            })],
            details: Some(json!({
                "mcp_tool": self.mcp_name,
                "truncated": bounded.truncated,
            })),
            is_error: result.is_error,
        })
    }
}

fn description_for(allowed: AllowedCodebaseMemoryTool, server_description: &str) -> String {
    let base = match server_description.trim() {
        "" => format!("Call codebase-memory MCP tool `{}`.", allowed.mcp_name),
        description => description.to_string(),
    };
    format!(
        "{base}\n\nRead-only wrapper around codebase-memory MCP tool `{}`.",
        allowed.mcp_name
    )
}

struct BoundedText {
    text: String,
    truncated: bool,
}

fn bound_text(input: &str, max_bytes: usize) -> BoundedText {
    if input.len() <= max_bytes {
        return BoundedText {
            text: input.to_string(),
            truncated: false,
        };
    }

    let notice = format!("\n[codebase-memory output truncated to {max_bytes} bytes]");
    let content_budget = max_bytes.saturating_sub(notice.len());
    let mut end = content_budget.min(input.len());
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    let mut text = input[..end].to_string();
    text.push_str(&notice);
    BoundedText {
        text,
        truncated: true,
    }
}

#[cfg(test)]
mod tests;
