//! Codebase-memory MCP allowlist and read-only tongs tool wrappers.
//!
//! The public entry point is [`build_codebase_memory_toolset`]: pass the parsed
//! worker→agent [`temper_protocol_agent::AgentToolConfig`], the current role,
//! and the prepared workspace scope, and it returns safe, prefixed, read-only
//! tools plus registration metadata used to decide whether concise prompt
//! guidance is relevant. Complete tool names, descriptions, and schemas remain
//! on the actual provider tool definitions and are not copied into prompts.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};
use temper_agent_core::{
    SAFE_GRAPH_CORRELATION_DETAIL_KEY, SAFE_TOOL_FAILURE_DETAIL_KEY, ToolFailureCategory,
    ToolFailureDiagnostic,
};
use temper_protocol_activity::{GraphCorrelationTargetKindV1, GraphCorrelationV1};
use temper_protocol_agent::{
    AgentToolConfig, CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
    WorkspaceContext,
};
use tongs::error::Result;
use tongs::model::{ContentBlock, TextContent};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolRegistry, ToolUpdate};

use crate::mcp::{
    MAX_MCP_RECORD_BYTES, McpError, McpToolDescriptor, StdioMcpClient, StdioMcpServerConfig,
};
use temper_agent_core::AgentContainmentContext;

mod background;
mod confirmation;
mod health;
mod indexing;
mod lifecycle_observability;
mod provider;
mod scope;
mod tool;
mod tool_schema;

use health::CodebaseMemoryHealth;
use indexing::prepare_indexes;
use lifecycle_observability::{
    DiscoveryEvidence, DiscoveryOutcome, FailureCategory, emit_discovery, emit_identity_selected,
};
use provider::validate_provider_contract;
use scope::{WorkspaceScope, discover_workspace_projects};
#[cfg(test)]
use tool::{
    classify_input_failure, classify_mcp_error, classify_provider_failure,
    codebase_memory_failure_output,
};
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

/// Registration metadata for one safe codebase-memory tool.
///
/// The description mirrors the actual provider tool definition for callers
/// that inspect the toolset. Prompt rendering treats this metadata only as
/// evidence that at least one safe tool was registered.
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

    /// Registration metadata for the safe tools exposed to the provider.
    /// Prompt rendering uses only whether this slice is empty; provider tool
    /// definitions remain the sole model-facing source of names/descriptions.
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
    build_codebase_memory_toolset_with_timeout(config, role, context, cwd, Duration::MAX).await
}

/// Builds the toolset while clamping model-visible MCP calls to the generic
/// agent tool deadline. Startup and index operations retain their narrower,
/// purpose-specific limits.
pub async fn build_codebase_memory_toolset_with_timeout(
    config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
    generic_tool_timeout: Duration,
) -> std::result::Result<CodebaseMemoryToolset, CodebaseMemoryToolsetError> {
    let containment = default_containment_context();
    build_codebase_memory_toolset_with_timeout_and_containment(
        config,
        role,
        context,
        cwd,
        generic_tool_timeout,
        &containment,
    )
    .await
}

fn default_containment_context() -> AgentContainmentContext {
    #[cfg(test)]
    {
        crate::containment_tests::containment_context()
    }
    #[cfg(not(test))]
    AgentContainmentContext::production(None)
}

pub(crate) async fn build_codebase_memory_toolset_with_timeout_and_containment(
    config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
    generic_tool_timeout: Duration,
    containment: &AgentContainmentContext,
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

    match start_toolset(
        codebase_memory,
        role,
        scope,
        generic_tool_timeout,
        containment,
    )
    .await
    {
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
    role: &str,
    mut scope: WorkspaceScope,
    generic_tool_timeout: Duration,
    containment: &AgentContainmentContext,
) -> std::result::Result<CodebaseMemoryToolset, McpError> {
    let startup_timeout = Duration::from_secs(config.startup_timeout_secs);
    let index_timeout = Duration::from_secs(config.index_timeout_secs);
    let call_timeout = effective_mcp_call_timeout(index_timeout, generic_tool_timeout);
    let mcp_config = StdioMcpServerConfig::new(config.command.clone(), config.args.clone())
        .with_containment_identity("codebase-memory")
        .with_startup_timeout(startup_timeout)
        .with_call_timeout(index_timeout);
    emit_agent_tool_configured(AgentToolConfigured {
        role,
        tool_name: "codebase_memory",
        mode: codebase_memory_mode(config.mode),
        index: codebase_memory_index(config.index),
        model_visible: false,
        repo_root: &scope.primary_root().display().to_string(),
    });
    let discovery_client =
        StdioMcpClient::connect_with_containment(mcp_config.clone(), containment.clone()).await?;
    let discovery_tools = discovery_client.list_tools(startup_timeout).await?;
    validate_provider_contract(&discovery_client, &discovery_tools)?;
    let mut setup_notes = Vec::new();

    let discovery_started = Instant::now();
    match discover_workspace_projects(&discovery_client, startup_timeout, &scope).await {
        Ok(states) => {
            let record_count = states.len();
            scope
                .apply_targeted_discovery(states)
                .map_err(McpError::Protocol)?;
            emit_discovery(DiscoveryEvidence {
                method: "index_status",
                inventory: "targeted",
                duration: discovery_started.elapsed(),
                outcome: DiscoveryOutcome::Success,
                record_count,
                cache_bytes: None,
                failure: FailureCategory::None,
            });
            for project in &scope.projects {
                let outcome = match project.index_state {
                    scope::ProjectIndexState::Missing => "missing",
                    scope::ProjectIndexState::Stale => "stale",
                    scope::ProjectIndexState::Fresh => "fresh",
                    _ => "unavailable",
                };
                emit_identity_selected(&project.canonical_alias, &project.provider_key, outcome);
            }
        }
        Err(error) if config.mode == CodebaseMemoryMode::Auto => {
            let outcome = if matches!(error, McpError::Timeout { .. }) {
                DiscoveryOutcome::Timeout
            } else {
                DiscoveryOutcome::Failure
            };
            emit_discovery(DiscoveryEvidence {
                method: "index_status",
                inventory: "targeted",
                duration: discovery_started.elapsed(),
                outcome,
                record_count: 0,
                cache_bytes: None,
                failure: FailureCategory::from(&error),
            });
            scope.mark_discovery_unavailable();
            setup_notes.push(
                "safe targeted project discovery was unavailable; indexing was skipped for every prepared repo and no path-keyed fallback was attempted"
                    .to_string(),
            );
        }
        Err(error) => {
            let outcome = if matches!(error, McpError::Timeout { .. }) {
                DiscoveryOutcome::Timeout
            } else {
                DiscoveryOutcome::Failure
            };
            emit_discovery(DiscoveryEvidence {
                method: "index_status",
                inventory: "targeted",
                duration: discovery_started.elapsed(),
                outcome,
                record_count: 0,
                cache_bytes: None,
                failure: FailureCategory::from(&error),
            });
            return Err(error);
        }
    }

    // Discovery requests and their timeouts are process-fatal in the stdio
    // client. Never clone that process into model-visible wrappers, even after
    // successful discovery: initialize and validate a fresh serving client.
    drop(discovery_client);
    let client =
        StdioMcpClient::connect_with_containment(mcp_config.clone(), containment.clone()).await?;
    let advertised = client.list_tools(startup_timeout).await?;
    validate_provider_contract(&client, &advertised)?;

    // Establish the serving client before spawning a background upsert. That
    // makes the background index's readiness visible from the first model tool
    // call instead of consuming its work while the serving client starts.
    setup_notes
        .extend(prepare_indexes(config, &mcp_config, &advertised, &mut scope, containment).await?);
    emit_mcp_server_started(McpServerStarted {
        tool_name: "codebase_memory",
        command: &config.command,
        repo_root: &scope.primary_root().display().to_string(),
    });
    scope.rebuild_alias_map();
    let prompt_status = scope.prompt_status(config.index, &setup_notes);
    let scope = Arc::new(scope);

    // This state belongs to exactly this toolset build (one agent run) and is
    // shared by every wrapper cloned from the serving client.
    let health = Arc::new(CodebaseMemoryHealth::new(client.cancellation_handle()));

    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    let mut registered_tool_metadata = Vec::new();

    for descriptor in advertised {
        let Some(allowed) = allowed_tool(&descriptor.name) else {
            let hidden_name = format!("codebase_memory_{}", descriptor.name);
            emit_agent_tool_hidden(AgentToolHidden {
                role,
                tool_name: &hidden_name,
                mcp_tool: &descriptor.name,
                model_visible: false,
                reason: "not on safe model allowlist",
            });
            continue;
        };
        let public_name = allowed.public_name.to_string();
        let default_project_key = default_project_key(allowed.mcp_name, &descriptor.input_schema);
        let description = description_for(*allowed, &descriptor.description, &scope);
        let parameters = scoped_parameters(&descriptor.input_schema, *allowed, &scope);
        emit_agent_tool_exposed(AgentToolExposed {
            role,
            tool_name: &public_name,
            mcp_tool: &descriptor.name,
            model_visible: true,
            repo_root: &scope.primary_root().display().to_string(),
            mcp_project: &scope.primary_actual_project(),
        });
        registered_tool_metadata.push(CodebaseMemoryToolMetadata {
            name: public_name.clone(),
            description: description.clone(),
        });
        tools.push(Box::new(CodebaseMemoryTool::new(
            client.clone(),
            Arc::clone(&health),
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

fn effective_mcp_call_timeout(index_timeout: Duration, generic_tool_timeout: Duration) -> Duration {
    index_timeout.min(generic_tool_timeout)
}

fn allowed_tool(name: &str) -> Option<&'static AllowedCodebaseMemoryTool> {
    ALLOWLIST.iter().find(|tool| tool.mcp_name == name)
}

fn advertised_tool(advertised: &[McpToolDescriptor], name: &str) -> bool {
    advertised.iter().any(|descriptor| descriptor.name == name)
}

struct AgentToolConfigured<'a> {
    role: &'a str,
    tool_name: &'a str,
    mode: &'a str,
    index: &'a str,
    model_visible: bool,
    repo_root: &'a str,
}

struct AgentToolExposed<'a> {
    role: &'a str,
    tool_name: &'a str,
    mcp_tool: &'a str,
    model_visible: bool,
    repo_root: &'a str,
    mcp_project: &'a str,
}

struct AgentToolHidden<'a> {
    role: &'a str,
    tool_name: &'a str,
    mcp_tool: &'a str,
    model_visible: bool,
    reason: &'a str,
}

struct McpServerStarted<'a> {
    tool_name: &'a str,
    command: &'a str,
    repo_root: &'a str,
}

struct McpToolCalled<'a> {
    tool_name: &'a str,
    mcp_tool: &'a str,
    mcp_project: &'a str,
    repo_root: &'a str,
    argument_preview: &'a str,
}

struct McpToolResult<'a> {
    tool_name: &'a str,
    mcp_tool: &'a str,
    mcp_project: &'a str,
    is_error: bool,
    truncated: bool,
    result_preview: &'a str,
    readiness_wait_ms: u64,
    graph_execution_ms: u64,
    duration_ms: u64,
    graph_correlation: Option<&'a GraphCorrelationV1>,
}

fn emit_agent_tool_configured(ev: AgentToolConfigured<'_>) {
    tracing::debug!(
        target: "temper::agent",
        service = "agent",
        event = "agent.tool.configured",
        role = ev.role,
        tool.name = ev.tool_name,
        tool.model_visible = ev.model_visible,
        mode = ev.mode,
        index = ev.index,
        repo.root = ev.repo_root,
        "agent:   tool configured: {} role={} mode={} index={}",
        ev.tool_name,
        ev.role,
        ev.mode,
        ev.index,
    );
}

fn emit_agent_tool_exposed(ev: AgentToolExposed<'_>) {
    tracing::debug!(
        target: "temper::agent",
        service = "agent",
        event = "agent.tool.exposed",
        role = ev.role,
        tool.name = ev.tool_name,
        mcp.tool = ev.mcp_tool,
        tool.model_visible = ev.model_visible,
        repo.root = ev.repo_root,
        mcp.project = ev.mcp_project,
        "agent:   tool exposed: {} -> {}",
        ev.tool_name,
        ev.mcp_tool,
    );
}

fn emit_agent_tool_hidden(ev: AgentToolHidden<'_>) {
    tracing::debug!(
        target: "temper::agent",
        service = "agent",
        event = "agent.tool.hidden",
        role = ev.role,
        tool.name = ev.tool_name,
        mcp.tool = ev.mcp_tool,
        tool.model_visible = ev.model_visible,
        reason = ev.reason,
        "agent:   tool hidden: {} ({})",
        ev.tool_name,
        ev.reason,
    );
}

fn emit_mcp_server_started(ev: McpServerStarted<'_>) {
    tracing::debug!(
        target: "temper::agent",
        service = "agent",
        event = "mcp.server.started",
        tool.name = ev.tool_name,
        command = ev.command,
        repo.root = ev.repo_root,
        "agent:   MCP server started: {}",
        ev.tool_name,
    );
}

fn emit_mcp_tool_called(ev: McpToolCalled<'_>) {
    tracing::debug!(
        target: "temper::agent",
        service = "agent",
        event = "mcp.tool.called",
        tool.name = ev.tool_name,
        mcp.tool = ev.mcp_tool,
        mcp.project = ev.mcp_project,
        repo.root = ev.repo_root,
        argument.preview = ev.argument_preview,
        "agent:   MCP tool called: {}",
        ev.mcp_tool,
    );
}

fn emit_mcp_tool_result(ev: McpToolResult<'_>) {
    // Scenario and operator logs need to distinguish a successful targeted
    // call with a complete typed correlation from a generic graph success. Do
    // not project the digest here: activity traces retain that opaque value for
    // the relevance analyzer, while live run evidence needs only aggregate
    // completion and closed type facts.
    let (correlation_version, correlation_tool, correlation_target_kind) = ev
        .graph_correlation
        .map(|correlation| {
            (
                correlation.version,
                correlation.tool.public_name(),
                graph_correlation_target_kind(correlation.target_kind),
            )
        })
        .unwrap_or((0, "", ""));
    tracing::debug!(
        target: "temper::agent",
        service = "agent",
        event = "mcp.tool.result",
        tool.name = ev.tool_name,
        mcp.tool = ev.mcp_tool,
        mcp.project = ev.mcp_project,
        is_error = ev.is_error,
        truncated = ev.truncated,
        result.preview = ev.result_preview,
        readiness.wait_ms = ev.readiness_wait_ms,
        graph.execution_ms = ev.graph_execution_ms,
        duration_ms = ev.duration_ms,
        graph.correlation.complete = ev.graph_correlation.is_some(),
        graph.correlation.version = correlation_version,
        graph.correlation.tool = correlation_tool,
        graph.correlation.target_kind = correlation_target_kind,
        "agent:   MCP tool result: {} error={}",
        ev.mcp_tool,
        ev.is_error,
    );
}

fn graph_correlation_target_kind(kind: GraphCorrelationTargetKindV1) -> &'static str {
    match kind {
        GraphCorrelationTargetKindV1::GraphQuery => "graph_query",
        GraphCorrelationTargetKindV1::Pattern => "pattern",
        GraphCorrelationTargetKindV1::NamePattern => "name_pattern",
        GraphCorrelationTargetKindV1::QualifiedNamePattern => "qualified_name_pattern",
        GraphCorrelationTargetKindV1::FunctionName => "function_name",
        GraphCorrelationTargetKindV1::QualifiedName => "qualified_name",
    }
}

fn codebase_memory_mode(mode: CodebaseMemoryMode) -> &'static str {
    match mode {
        CodebaseMemoryMode::Auto => "auto",
        CodebaseMemoryMode::Required => "required",
    }
}

fn codebase_memory_index(index: CodebaseMemoryIndex) -> &'static str {
    match index {
        CodebaseMemoryIndex::Off => "off",
        CodebaseMemoryIndex::Background => "background",
        CodebaseMemoryIndex::Blocking => "blocking",
    }
}

struct CodebaseMemoryTool {
    client: StdioMcpClient,
    health: Arc<CodebaseMemoryHealth>,
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
        health: Arc<CodebaseMemoryHealth>,
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
            health,
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

#[cfg(test)]
mod tests;
