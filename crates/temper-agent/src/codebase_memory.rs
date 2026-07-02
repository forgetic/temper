//! Codebase-memory MCP allowlist and read-only tongs tool wrappers.
//!
//! The public entry point for the next coding-agent wiring slice is
//! [`build_codebase_memory_toolset`]: pass the parsed worker→agent
//! [`temper_protocol_agent::AgentToolConfig`] and the current role, and it
//! returns a set of safe, prefixed, read-only tools plus metadata for prompt
//! generation. The current coding-agent run path does not call this module yet.

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
    tools: Vec<Box<dyn Tool>>,
}

impl CodebaseMemoryToolset {
    fn disabled(status: CodebaseMemoryToolsetStatus) -> Self {
        Self {
            status,
            registered_tool_names: Vec::new(),
            tools: Vec::new(),
        }
    }

    fn started(tools: Vec<Box<dyn Tool>>, registered_tool_names: Vec<String>) -> Self {
        Self {
            status: CodebaseMemoryToolsetStatus::Started,
            registered_tool_names,
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
    let mut registered_tool_names = Vec::new();

    for descriptor in advertised {
        let Some(allowed) = allowed_tool(&descriptor.name) else {
            continue;
        };
        registered_tool_names.push(allowed.public_name.to_string());
        tools.push(Box::new(CodebaseMemoryTool::new(
            client.clone(),
            descriptor,
            *allowed,
            call_timeout,
        )));
    }

    Ok(CodebaseMemoryToolset::started(tools, registered_tool_names))
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
        call_timeout: Duration,
    ) -> Self {
        Self {
            client,
            mcp_name: descriptor.name,
            public_name: allowed.public_name.to_string(),
            description: description_for(allowed, &descriptor.description),
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
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fake_server_script() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("fake_codebase_memory_mcp.py"),
            r#"
import json
import sys
import time

mode = sys.argv[1] if len(sys.argv) > 1 else "normal"
if mode == "hang":
    time.sleep(60)
    sys.exit(0)

TOOLS = [
    {"name": "search_code", "description": "Search indexed code", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
    {"name": "get_architecture", "description": "Summarize architecture", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "delete_project", "description": "Delete project", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "manage_adr", "description": "Write ADRs", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "ingest_traces", "description": "Ingest traces", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "query_graph", "description": "Raw graph query", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "index_repository", "description": "Index arbitrary path", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}}},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": "2024-11-05", "serverInfo": {"name": "fake-codebase-memory", "version": "1"}, "capabilities": {"tools": {}}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params", {})
        name = params.get("name")
        args = params.get("arguments") or {}
        payload = f"{name} result for {json.dumps(args, sort_keys=True)}\n" + ("x" * 20000)
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": payload}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#,
        )
        .expect("write fake server");
        dir
    }

    fn script_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("fake_codebase_memory_mcp.py")
    }

    fn config(dir: &tempfile::TempDir, mode: CodebaseMemoryMode) -> AgentToolConfig {
        AgentToolConfig {
            codebase_memory: Some(CodebaseMemoryToolConfig {
                mode,
                command: "python3".to_string(),
                args: vec!["-u".to_string(), script_path(dir).display().to_string()],
                roles: vec!["engineer".to_string()],
                index: temper_protocol_agent::CodebaseMemoryIndex::Off,
                startup_timeout_secs: 1,
                index_timeout_secs: 2,
            }),
        }
    }

    fn hanging_config(dir: &tempfile::TempDir, mode: CodebaseMemoryMode) -> AgentToolConfig {
        let mut config = config(dir, mode);
        let codebase_memory = config
            .codebase_memory
            .as_mut()
            .expect("codebase memory config");
        codebase_memory.args.push("hang".to_string());
        config
    }

    fn output_text(output: &ToolOutput) -> String {
        output
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn codebase_memory_bridge_wraps_allowed_tool_and_filters_destructive_tools() {
        let dir = fake_server_script();
        temper_agent_io::block_on(async move {
            let toolset = build_codebase_memory_toolset(
                Some(&config(&dir, CodebaseMemoryMode::Required)),
                "engineer",
            )
            .await
            .expect("build required codebase-memory toolset");
            assert_eq!(toolset.status(), &CodebaseMemoryToolsetStatus::Started);
            let names = toolset.registered_tool_names().to_vec();
            assert!(names.contains(&"codebase_memory_search_code".to_string()));
            assert!(names.contains(&"codebase_memory_get_architecture".to_string()));
            for forbidden in [
                "codebase_memory_delete_project",
                "codebase_memory_manage_adr",
                "codebase_memory_ingest_traces",
                "codebase_memory_query_graph",
                "codebase_memory_index_repository",
            ] {
                assert!(
                    !names.contains(&forbidden.to_string()),
                    "{forbidden} must not be registered"
                );
            }

            let tools = toolset.into_tools();
            let search = tools
                .iter()
                .find(|tool| tool.name() == "codebase_memory_search_code")
                .expect("search wrapper present");
            assert_eq!(search.effects(), ToolEffects::read());
            let output = search
                .execute("call-1", json!({ "query": "needle" }), None)
                .await
                .expect("execute wrapped MCP tool");
            let text = output_text(&output);
            assert!(!output.is_error);
            assert!(text.contains("search_code result"));
            assert!(text.contains("needle"));
            assert!(text.contains("output truncated"));
            assert!(text.len() <= MAX_CODEBASE_MEMORY_OUTPUT_BYTES);
        });
    }

    #[test]
    fn codebase_memory_bridge_auto_vs_required_startup_failures() {
        let auto = AgentToolConfig {
            codebase_memory: Some(CodebaseMemoryToolConfig {
                mode: CodebaseMemoryMode::Auto,
                command: "definitely-not-a-temper-codebase-memory-mcp".to_string(),
                args: Vec::new(),
                roles: vec!["engineer".to_string()],
                index: temper_protocol_agent::CodebaseMemoryIndex::Off,
                startup_timeout_secs: 1,
                index_timeout_secs: 1,
            }),
        };
        let required = AgentToolConfig {
            codebase_memory: Some(CodebaseMemoryToolConfig {
                mode: CodebaseMemoryMode::Required,
                command: "definitely-not-a-temper-codebase-memory-mcp".to_string(),
                args: Vec::new(),
                roles: vec!["engineer".to_string()],
                index: temper_protocol_agent::CodebaseMemoryIndex::Off,
                startup_timeout_secs: 1,
                index_timeout_secs: 1,
            }),
        };

        temper_agent_io::block_on(async move {
            let auto_toolset = build_codebase_memory_toolset(Some(&auto), "engineer")
                .await
                .expect("auto mode suppresses startup failure");
            assert!(matches!(
                auto_toolset.status(),
                CodebaseMemoryToolsetStatus::AutoUnavailable { reason }
                    if reason.contains("spawn MCP command")
            ));
            assert!(auto_toolset.registered_tool_names().is_empty());

            let required_error =
                match build_codebase_memory_toolset(Some(&required), "engineer").await {
                    Ok(_) => panic!("required mode hard-fails startup failure"),
                    Err(error) => error,
                };
            assert!(
                required_error
                    .to_string()
                    .contains("required codebase-memory MCP startup failed")
            );
        });
    }

    #[test]
    fn codebase_memory_bridge_auto_timeout_is_best_effort_required_timeout_fails() {
        let dir = fake_server_script();
        temper_agent_io::block_on(async move {
            let auto = hanging_config(&dir, CodebaseMemoryMode::Auto);
            let auto_toolset = build_codebase_memory_toolset(Some(&auto), "engineer")
                .await
                .expect("auto mode suppresses timeout");
            assert!(matches!(
                auto_toolset.status(),
                CodebaseMemoryToolsetStatus::AutoUnavailable { reason }
                    if reason.contains("timed out")
            ));

            let required = hanging_config(&dir, CodebaseMemoryMode::Required);
            let error = match build_codebase_memory_toolset(Some(&required), "engineer").await {
                Ok(_) => panic!("required mode fails timeout"),
                Err(error) => error,
            };
            assert!(error.to_string().contains("timed out"));
        });
    }
}
