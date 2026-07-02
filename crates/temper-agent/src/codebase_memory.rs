//! Codebase-memory MCP allowlist and read-only tongs tool wrappers.
//!
//! The public entry point is [`build_codebase_memory_toolset`]: pass the parsed
//! worker→agent [`temper_protocol_agent::AgentToolConfig`], the current role,
//! and the prepared workspace scope, and it returns safe, prefixed, read-only
//! tools plus metadata for prompt generation.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use temper_protocol_agent::{
    AgentToolConfig, CodebaseMemoryMode, CodebaseMemoryToolConfig, WorkspaceContext,
};
use tongs::error::{Error, Result};
use tongs::model::{ContentBlock, TextContent};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolRegistry, ToolUpdate};

use crate::mcp::{McpError, McpToolDescriptor, StdioMcpClient, StdioMcpServerConfig};

mod indexing;
mod scope;
mod tool_schema;

use indexing::{discover_indexed_projects, prepare_indexes};
use scope::WorkspaceScope;
use tool_schema::{default_project_key, description_for, scoped_parameters};

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
    prompt_status: Option<String>,
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
            prompt_status: None,
            tools: Vec::new(),
        }
    }

    fn started(
        tools: Vec<Box<dyn Tool>>,
        registered_tool_metadata: Vec<CodebaseMemoryToolMetadata>,
        prompt_status: String,
    ) -> Self {
        let registered_tool_names = registered_tool_metadata
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        Self {
            status: CodebaseMemoryToolsetStatus::Started,
            registered_tool_names,
            registered_tool_metadata,
            prompt_status: Some(prompt_status),
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

    /// Workspace/index status rendered into the coding-agent prompt when tools
    /// are registered.
    pub fn prompt_status(&self) -> Option<&str> {
        self.prompt_status.as_deref()
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

/// Hard-fail error returned only for `required` mode startup/list/index failures.
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

    fn required_setup(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CodebaseMemoryToolsetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CodebaseMemoryToolsetError {}

/// Builds a codebase-memory MCP toolset from the parsed agent tool config and
/// prepared workspace scope.
///
/// Error behavior is mode-dependent:
///
/// - absent config or a role mismatch returns an empty, disabled toolset;
/// - `mode = auto` returns an empty `AutoUnavailable` toolset on MCP
///   spawn/initialize/list failure;
/// - `mode = required` returns [`CodebaseMemoryToolsetError`] for those same
///   startup failures;
/// - index/bootstrap failures are fatal only in `required`; in `auto` the tools
///   are exposed with stale/in-progress prompt metadata.
///
/// The agent-callable tools are workspace-scoped: `project`/`repo` inputs are
/// resolved only against aliases derived from [`WorkspaceContext::repos`], and
/// internal `index_repository` calls are made only for those prepared repo roots.
pub async fn build_codebase_memory_toolset(
    config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
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

    let scope = match WorkspaceScope::from_context(context, cwd) {
        Ok(scope) => scope,
        Err(error) if codebase_memory.mode == CodebaseMemoryMode::Auto => {
            return Ok(CodebaseMemoryToolset::disabled(
                CodebaseMemoryToolsetStatus::AutoUnavailable { reason: error },
            ));
        }
        Err(error) => {
            return Err(CodebaseMemoryToolsetError::required_setup(format!(
                "required codebase-memory workspace scope failed: {error}"
            )));
        }
    };

    match start_toolset(codebase_memory, scope).await {
        Ok(toolset) => Ok(toolset),
        Err(error) if codebase_memory.mode == CodebaseMemoryMode::Auto => Ok(
            CodebaseMemoryToolset::disabled(CodebaseMemoryToolsetStatus::AutoUnavailable {
                reason: error.to_string(),
            }),
        ),
        Err(error) => Err(CodebaseMemoryToolsetError::required_startup(error)),
    }
}

async fn start_toolset(
    config: &CodebaseMemoryToolConfig,
    mut scope: WorkspaceScope,
) -> std::result::Result<CodebaseMemoryToolset, McpError> {
    let startup_timeout = Duration::from_secs(config.startup_timeout_secs);
    let call_timeout = Duration::from_secs(config.index_timeout_secs);
    let mcp_config = StdioMcpServerConfig::new(config.command.clone(), config.args.clone())
        .with_startup_timeout(startup_timeout)
        .with_call_timeout(call_timeout);
    let client = StdioMcpClient::connect(mcp_config.clone()).await?;
    let advertised = client.list_tools(startup_timeout).await?;
    let mut setup_notes = Vec::new();

    if advertised_tool(&advertised, "list_projects") {
        match discover_indexed_projects(&client, startup_timeout).await {
            Ok(discovered) => scope.apply_discovered_projects(discovered, true),
            Err(error) if config.mode == CodebaseMemoryMode::Auto => {
                setup_notes.push(format!(
                    "could not read codebase-memory project list; aliases use prepared repo names only: {error}"
                ));
                scope.apply_discovered_projects(Vec::new(), false);
            }
            Err(error) => return Err(error),
        }
    } else {
        scope.apply_discovered_projects(Vec::new(), false);
    }

    setup_notes
        .extend(prepare_indexes(config, &client, &mcp_config, &advertised, &mut scope).await?);
    scope.rebuild_alias_map();
    let prompt_status = scope.prompt_status(config.index, &setup_notes);
    let scope = Arc::new(scope);

    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    let mut registered_tool_metadata = Vec::new();

    for descriptor in advertised {
        let Some(allowed) = allowed_tool(&descriptor.name) else {
            continue;
        };
        let public_name = allowed.public_name.to_string();
        let default_project_key = default_project_key(allowed.mcp_name, &descriptor.input_schema);
        let description = description_for(*allowed, &descriptor.description, &scope);
        let parameters = scoped_parameters(&descriptor.input_schema, *allowed, &scope);
        registered_tool_metadata.push(CodebaseMemoryToolMetadata {
            name: public_name.clone(),
            description: description.clone(),
        });
        tools.push(Box::new(CodebaseMemoryTool::new(
            client.clone(),
            descriptor.name,
            *allowed,
            public_name,
            description,
            parameters,
            default_project_key,
            call_timeout,
            Arc::clone(&scope),
        )));
    }

    Ok(CodebaseMemoryToolset::started(
        tools,
        registered_tool_metadata,
        prompt_status,
    ))
}

fn allowed_tool(name: &str) -> Option<&'static AllowedCodebaseMemoryTool> {
    ALLOWLIST.iter().find(|tool| tool.mcp_name == name)
}

fn advertised_tool(advertised: &[McpToolDescriptor], name: &str) -> bool {
    advertised.iter().any(|descriptor| descriptor.name == name)
}

struct CodebaseMemoryTool {
    client: StdioMcpClient,
    mcp_name: String,
    public_name: String,
    description: String,
    parameters: Value,
    default_project_key: Option<&'static str>,
    call_timeout: Duration,
    scope: Arc<WorkspaceScope>,
}

impl CodebaseMemoryTool {
    #[allow(clippy::too_many_arguments)]
    fn new(
        client: StdioMcpClient,
        mcp_name: String,
        allowed: AllowedCodebaseMemoryTool,
        public_name: String,
        description: String,
        parameters: Value,
        default_project_key: Option<&'static str>,
        call_timeout: Duration,
        scope: Arc<WorkspaceScope>,
    ) -> Self {
        debug_assert_eq!(public_name, allowed.public_name);
        Self {
            client,
            mcp_name,
            public_name,
            description,
            parameters,
            default_project_key,
            call_timeout,
            scope,
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
        let input = self
            .scope
            .prepare_tool_input(&self.mcp_name, self.default_project_key, input)
            .map_err(|message| Error::tool(self.public_name.clone(), message))?;

        if self.mcp_name == "list_projects" {
            return Ok(self.scope.list_projects_output());
        }

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
                "workspace_scope": self.scope.details_json(),
            })),
            is_error: result.is_error,
        })
    }
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
