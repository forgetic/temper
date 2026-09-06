//! Closed tool lifecycle and graph result event projection.
use super::*;

pub(super) struct AgentToolConfigured<'a> {
    pub(super) role: &'a str,
    pub(super) tool_name: &'a str,
    pub(super) mode: &'a str,
    pub(super) index: &'a str,
    pub(super) model_visible: bool,
    pub(super) repo_root: &'a str,
}

pub(super) struct AgentToolExposed<'a> {
    pub(super) role: &'a str,
    pub(super) tool_name: &'a str,
    pub(super) mcp_tool: &'a str,
    pub(super) model_visible: bool,
    pub(super) repo_root: &'a str,
    pub(super) mcp_project: &'a str,
}

pub(super) struct AgentToolHidden<'a> {
    pub(super) role: &'a str,
    pub(super) tool_name: &'a str,
    pub(super) mcp_tool: &'a str,
    pub(super) model_visible: bool,
    pub(super) reason: &'a str,
}

pub(super) struct McpServerStarted<'a> {
    pub(super) tool_name: &'a str,
    pub(super) command: &'a str,
    pub(super) repo_root: &'a str,
}

pub(super) struct McpToolCalled<'a> {
    pub(super) tool_name: &'a str,
    pub(super) mcp_tool: &'a str,
    pub(super) mcp_project: &'a str,
    pub(super) repo_root: &'a str,
    pub(super) argument_preview: &'a str,
}

pub(super) struct McpToolResult<'a> {
    pub(super) tool_name: &'a str,
    pub(super) mcp_tool: &'a str,
    pub(super) mcp_project: &'a str,
    pub(super) is_error: bool,
    pub(super) truncated: bool,
    pub(super) result_preview: &'a str,
    pub(super) readiness_wait_ms: u64,
    pub(super) graph_execution_ms: u64,
    pub(super) duration_ms: u64,
    pub(super) graph_correlation: Option<&'a GraphCorrelationV1>,
    pub(super) decision_anchor_lineage: Option<&'a DecisionAnchorLineageV1>,
}

pub(super) fn emit_agent_tool_configured(ev: AgentToolConfigured<'_>) {
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

pub(super) fn emit_agent_tool_exposed(ev: AgentToolExposed<'_>) {
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

pub(super) fn emit_agent_tool_hidden(ev: AgentToolHidden<'_>) {
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

pub(super) fn emit_mcp_server_started(ev: McpServerStarted<'_>) {
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

pub(super) fn emit_mcp_tool_called(ev: McpToolCalled<'_>) {
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

pub(super) fn emit_mcp_tool_result(ev: McpToolResult<'_>) {
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
    let focused_test_discovery = ev
        .decision_anchor_lineage
        .and_then(|lineage| lineage.focused_test_discovery)
        .map(focused_test_discovery_outcome)
        .unwrap_or("");
    let lineage_evidence_kind = ev
        .decision_anchor_lineage
        .and_then(|lineage| lineage.decision_evidence_kind)
        .map(decision_evidence_kind)
        .unwrap_or("");
    let implementation_correction_available = ev
        .decision_anchor_lineage
        .is_some_and(|lineage| lineage.implementation_correction_available);
    let implementation_authority_corrected = ev
        .decision_anchor_lineage
        .is_some_and(|lineage| lineage.implementation_authority_corrected);
    let (lineage_version, lineage_stage, lineage_result_target_kind_count) = ev
        .decision_anchor_lineage
        .map(|lineage| {
            (
                lineage.version,
                graph_lineage_stage(lineage.stage),
                lineage.result_target_kinds.len() as u64,
            )
        })
        .unwrap_or((0, "", 0));
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
        graph.lineage.complete = ev.decision_anchor_lineage.is_some(),
        graph.lineage.version = lineage_version,
        graph.lineage.stage = lineage_stage,
        graph.lineage.result_target_kind_count = lineage_result_target_kind_count,
        graph.lineage.decision_evidence_kind = lineage_evidence_kind,
        graph.lineage.focused_test_discovery = focused_test_discovery,
        graph.lineage.implementation_correction_available = implementation_correction_available,
        graph.lineage.implementation_authority_corrected = implementation_authority_corrected,
        "agent:   MCP tool result: {} error={}",
        ev.mcp_tool,
        ev.is_error,
    );
}

pub(super) fn graph_correlation_target_kind(kind: GraphCorrelationTargetKindV1) -> &'static str {
    match kind {
        GraphCorrelationTargetKindV1::GraphQuery => "graph_query",
        GraphCorrelationTargetKindV1::Pattern => "pattern",
        GraphCorrelationTargetKindV1::NamePattern => "name_pattern",
        GraphCorrelationTargetKindV1::QualifiedNamePattern => "qualified_name_pattern",
        GraphCorrelationTargetKindV1::FunctionName => "function_name",
        GraphCorrelationTargetKindV1::QualifiedName => "qualified_name",
    }
}

pub(super) fn graph_lineage_stage(stage: DecisionAnchorLineageStageV1) -> &'static str {
    match stage {
        DecisionAnchorLineageStageV1::Root => "root",
        DecisionAnchorLineageStageV1::CarryForward => "carry_forward",
    }
}

pub(super) fn decision_evidence_kind(kind: DecisionEvidenceKindV1) -> &'static str {
    match kind {
        DecisionEvidenceKindV1::Implementation => "implementation",
        DecisionEvidenceKindV1::Caller => "caller",
        DecisionEvidenceKindV1::FocusedTest => "focused_test",
    }
}

pub(super) fn focused_test_discovery_outcome(
    outcome: temper_protocol_activity::FocusedTestDiscoveryOutcomeV1,
) -> &'static str {
    match outcome {
        temper_protocol_activity::FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned => {
            "eligible_selector_returned"
        }
        temper_protocol_activity::FocusedTestDiscoveryOutcomeV1::NoEligibleSelector => {
            "no_eligible_selector"
        }
    }
}

pub(super) fn codebase_memory_mode(mode: CodebaseMemoryMode) -> &'static str {
    match mode {
        CodebaseMemoryMode::Auto => "auto",
        CodebaseMemoryMode::Required => "required",
    }
}

pub(super) fn codebase_memory_index(index: CodebaseMemoryIndex) -> &'static str {
    match index {
        CodebaseMemoryIndex::Off => "off",
        CodebaseMemoryIndex::Background => "background",
        CodebaseMemoryIndex::Blocking => "blocking",
    }
}
