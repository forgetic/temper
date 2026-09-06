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
    LineageAdmissionHandle, SAFE_GRAPH_CORRELATION_DETAIL_KEY, SAFE_TOOL_FAILURE_DETAIL_KEY,
    ToolFailureCategory, ToolFailureDiagnostic,
};
use temper_protocol_activity::{
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionEvidenceKindV1,
    GraphCorrelationTargetKindV1, GraphCorrelationV1,
};
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
mod coverage;
pub(crate) use coverage::{CoverageService, GraphHandoffTool};
mod health;
mod indexing;
mod lifecycle_observability;
mod lineage;
mod provider;
mod provider_output;
mod result_presentation;
mod scope;
mod startup;
#[cfg(test)]
use startup::effective_mcp_call_timeout;
use startup::{advertised_tool, start_toolset};
mod tool;
mod tool_observability;
mod tool_schema;
use tool_observability::*;

use health::CodebaseMemoryHealth;
use indexing::prepare_indexes;
use lifecycle_observability::{
    DiscoveryEvidence, DiscoveryOutcome, FailureCategory, emit_discovery, emit_identity_selected,
};
use lineage::DecisionAnchorLineageRegistry;
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
    AllowedCodebaseMemoryTool::new(
        "check_index_coverage",
        "codebase_memory_check_index_coverage",
    ),
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
    lineage_admission: Option<LineageAdmissionHandle>,
    coverage: Option<Arc<CoverageService>>,
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
            lineage_admission: None,
            coverage: None,
        }
    }

    fn started(
        tools: Vec<Box<dyn Tool>>,
        registered_tool_metadata: Vec<CodebaseMemoryToolMetadata>,
        prompt_status: String,
        lineage_admission: LineageAdmissionHandle,
        coverage: Option<Arc<CoverageService>>,
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
            lineage_admission: Some(lineage_admission),
            coverage,
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

    /// Shared run-local pre-provider lineage resolver, when graph tools exist.
    pub fn lineage_admission(&self) -> Option<LineageAdmissionHandle> {
        self.lineage_admission.clone()
    }

    pub(crate) fn coverage(&self) -> Option<Arc<CoverageService>> {
        self.coverage.clone()
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
    decision_anchor_lineages: Arc<DecisionAnchorLineageRegistry>,
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
        decision_anchor_lineages: Arc<DecisionAnchorLineageRegistry>,
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
            decision_anchor_lineages,
        }
    }
}

#[cfg(test)]
mod tests;
