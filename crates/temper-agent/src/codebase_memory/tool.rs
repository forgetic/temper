use super::*;
use temper_protocol_activity::{
    GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
};

// Reserve space for the JSON-RPC envelope, tool name, request id, and newline.
// This keeps oversized model input on the local side of the process-fatal MCP
// record bound.
const MCP_REQUEST_ENVELOPE_RESERVE_BYTES: usize = 4 * 1024;

#[derive(Clone, Copy, Debug)]
struct MonotonicCallBudget {
    started: Instant,
    timeout: Duration,
}

impl MonotonicCallBudget {
    fn new(timeout: Duration) -> Self {
        Self {
            started: Instant::now(),
            timeout,
        }
    }

    fn remaining(self) -> Option<Duration> {
        self.timeout
            .checked_sub(self.started.elapsed())
            .filter(|remaining| !remaining.is_zero())
    }

    fn elapsed_ms(self) -> u64 {
        duration_ms(self.started.elapsed())
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
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
        let budget = MonotonicCallBudget::new(self.call_timeout);
        if let Some(cause) = self.health.open_cause() {
            return Ok(self.circuit_open_output(cause, ToolCallTimings::default()));
        }

        let scope = Arc::clone(&self.scope);
        let mcp_name = self.mcp_name.clone();
        let default_project_key = self.default_project_key;
        let readiness_started = Instant::now();
        let Some(readiness_budget) = budget.remaining().and_then(readiness_budget) else {
            let timings = ToolCallTimings {
                duration_ms: budget.elapsed_ms(),
                ..ToolCallTimings::default()
            };
            return Ok(self.failed_output("", ToolFailureCategory::Timeout, timings));
        };
        let input = match skein::runtime::spawn_blocking(move || {
            scope.prepare_tool_input(&mcp_name, default_project_key, input, readiness_budget)
        })
        .await
        {
            Ok(input) => input,
            Err(message) => {
                let timings = ToolCallTimings {
                    readiness_wait_ms: duration_ms(readiness_started.elapsed()),
                    graph_execution_ms: 0,
                    duration_ms: budget.elapsed_ms(),
                };
                let category = classify_input_failure(&message);
                return Ok(self.failed_output("", category, timings));
            }
        };
        let readiness_wait_ms = duration_ms(readiness_started.elapsed());
        // Extract only one closed, structured correlation target before the
        // provider consumes the input. The returned DTO is a digest, never a
        // raw model argument.
        let graph_correlation = graph_correlation(&self.public_name, &input);

        if self.mcp_name == "list_projects" {
            return Ok(self.scope.list_projects_output());
        }

        let mcp_project = input
            .get("project")
            .or_else(|| input.get("repo"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let serialized_input = input.to_string();
        if serialized_input.len()
            > MAX_MCP_RECORD_BYTES.saturating_sub(MCP_REQUEST_ENVELOPE_RESERVE_BYTES)
        {
            let timings = ToolCallTimings {
                readiness_wait_ms,
                graph_execution_ms: 0,
                duration_ms: budget.elapsed_ms(),
            };
            return Ok(self.failed_output(
                &mcp_project,
                ToolFailureCategory::InvalidModelInput,
                timings,
            ));
        }

        let Some(gate_budget) = budget.remaining().and_then(completion_budget) else {
            let timings = ToolCallTimings {
                readiness_wait_ms,
                graph_execution_ms: 0,
                duration_ms: budget.elapsed_ms(),
            };
            return Ok(self.failed_output(&mcp_project, ToolFailureCategory::Timeout, timings));
        };
        let _rpc_guard = match temper_agent_io::timeout(gate_budget, self.health.acquire_rpc())
            .await
        {
            Ok(guard) => guard,
            Err(_) => {
                let timings = ToolCallTimings {
                    readiness_wait_ms,
                    graph_execution_ms: 0,
                    duration_ms: budget.elapsed_ms(),
                };
                return Ok(self.failed_output(&mcp_project, ToolFailureCategory::Timeout, timings));
            }
        };
        if let Some(cause) = self.health.open_cause() {
            let timings = ToolCallTimings {
                readiness_wait_ms,
                graph_execution_ms: 0,
                duration_ms: budget.elapsed_ms(),
            };
            return Ok(self.circuit_open_output(cause, timings));
        }

        let Some(execution_budget) = budget.remaining().and_then(completion_budget) else {
            let timings = ToolCallTimings {
                readiness_wait_ms,
                graph_execution_ms: 0,
                duration_ms: budget.elapsed_ms(),
            };
            return Ok(self.failed_output(&mcp_project, ToolFailureCategory::Timeout, timings));
        };
        emit_mcp_tool_called(McpToolCalled {
            tool_name: &self.public_name,
            mcp_tool: &self.mcp_name,
            mcp_project: &mcp_project,
            repo_root: &self.scope.primary_root().display().to_string(),
            argument_preview: "<arguments omitted>",
        });

        let graph_started = Instant::now();
        let result = match temper_agent_io::timeout(
            execution_budget,
            self.client
                .call_tool(&self.mcp_name, input, execution_budget),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                let timings = ToolCallTimings {
                    readiness_wait_ms,
                    graph_execution_ms: duration_ms(graph_started.elapsed()),
                    duration_ms: budget.elapsed_ms(),
                };
                let category = classify_mcp_error(&error);
                return Ok(self.failed_output(&mcp_project, category, timings));
            }
            Err(_) => {
                let timings = ToolCallTimings {
                    readiness_wait_ms,
                    graph_execution_ms: duration_ms(graph_started.elapsed()),
                    duration_ms: budget.elapsed_ms(),
                };
                return Ok(self.failed_output(&mcp_project, ToolFailureCategory::Timeout, timings));
            }
        };
        let timings = ToolCallTimings {
            readiness_wait_ms,
            graph_execution_ms: duration_ms(graph_started.elapsed()),
            duration_ms: budget.elapsed_ms(),
        };
        let bounded = bound_text(&result.text, MAX_CODEBASE_MEMORY_OUTPUT_BYTES);
        if result.is_error {
            let category = classify_provider_failure(&bounded.text);
            return Ok(self.failed_output(&mcp_project, category, timings));
        }

        emit_mcp_tool_result(McpToolResult {
            tool_name: &self.public_name,
            mcp_tool: &self.mcp_name,
            mcp_project: &mcp_project,
            is_error: false,
            truncated: bounded.truncated,
            result_preview: "<result omitted>",
            readiness_wait_ms: timings.readiness_wait_ms,
            graph_execution_ms: timings.graph_execution_ms,
            duration_ms: timings.duration_ms,
            graph_correlation: graph_correlation.as_ref(),
        });
        let mut details = json!({
            "mcp_tool": self.mcp_name,
            "truncated": bounded.truncated,
            "workspace_scope": self.scope.details_json(),
            "timing": {
                "readiness_wait_ms": timings.readiness_wait_ms,
                "graph_execution_ms": timings.graph_execution_ms,
                "duration_ms": timings.duration_ms,
            },
        });
        if let Some(correlation) = graph_correlation {
            details[SAFE_GRAPH_CORRELATION_DETAIL_KEY] =
                serde_json::to_value(correlation).expect("graph correlation serializes");
        }
        Ok(ToolOutput {
            content: vec![ContentBlock::Text(TextContent {
                text: bounded.text,
                text_signature: None,
            })],
            details: Some(details),
            is_error: false,
        })
    }
}

impl CodebaseMemoryTool {
    fn failed_output(
        &self,
        mcp_project: &str,
        category: ToolFailureCategory,
        timings: ToolCallTimings,
    ) -> ToolOutput {
        self.health.record_failure(category);
        emit_failed_mcp_tool_result(self, mcp_project, category, timings);
        codebase_memory_failure_output_with_timings(category, timings, None)
    }

    fn circuit_open_output(
        &self,
        cause: ToolFailureCategory,
        timings: ToolCallTimings,
    ) -> ToolOutput {
        emit_failed_mcp_tool_result(self, "", ToolFailureCategory::CircuitOpen, timings);
        codebase_memory_failure_output_with_timings(
            ToolFailureCategory::CircuitOpen,
            timings,
            Some(cause),
        )
    }
}

fn completion_budget(remaining: Duration) -> Option<Duration> {
    // Leave a small completion margin so the typed result wins the generic
    // shell timeout instead of being dropped at an equal deadline.
    let margin = (remaining / 20).min(Duration::from_millis(10));
    let budget = remaining.saturating_sub(margin);
    (!budget.is_zero()).then_some(budget)
}

fn readiness_budget(remaining: Duration) -> Option<Duration> {
    // Readiness may legitimately consume almost the whole call budget. Keep a
    // narrow margin only for projecting its typed not-ready/index result.
    let margin = (remaining / 100).min(Duration::from_millis(2));
    let budget = remaining.saturating_sub(margin);
    (!budget.is_zero()).then_some(budget)
}

struct BoundedText {
    text: String,
    truncated: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct ToolCallTimings {
    readiness_wait_ms: u64,
    graph_execution_ms: u64,
    duration_ms: u64,
}

fn emit_failed_mcp_tool_result(
    tool: &CodebaseMemoryTool,
    mcp_project: &str,
    category: ToolFailureCategory,
    timings: ToolCallTimings,
) {
    emit_mcp_tool_result(McpToolResult {
        tool_name: &tool.public_name,
        mcp_tool: &tool.mcp_name,
        mcp_project,
        is_error: true,
        truncated: false,
        result_preview: category.safe_message(),
        readiness_wait_ms: timings.readiness_wait_ms,
        graph_execution_ms: timings.graph_execution_ms,
        duration_ms: timings.duration_ms,
        graph_correlation: None,
    });
}

#[cfg(test)]
pub(super) fn codebase_memory_failure_output(category: ToolFailureCategory) -> ToolOutput {
    codebase_memory_failure_output_with_timings(category, ToolCallTimings::default(), None)
}

fn codebase_memory_failure_output_with_timings(
    category: ToolFailureCategory,
    timings: ToolCallTimings,
    circuit_cause: Option<ToolFailureCategory>,
) -> ToolOutput {
    let diagnostic = ToolFailureDiagnostic::codebase_memory(category);
    let text = format!(
        "{}; do not retry codebase-memory immediately; continue with read, grep, find, shell, or other conventional discovery instead",
        diagnostic.message
    );
    let mut details = json!({
        SAFE_TOOL_FAILURE_DETAIL_KEY: {
            "source": "codebase_memory",
            "category": category.as_str(),
        },
        "timing": {
            "readiness_wait_ms": timings.readiness_wait_ms,
            "graph_execution_ms": timings.graph_execution_ms,
            "duration_ms": timings.duration_ms,
        },
    });
    if let Some(cause) = circuit_cause {
        details["circuit"] = json!({
            "scope": "codebase_memory_toolset_run",
            "opened_by": cause.as_str(),
        });
    }
    ToolOutput {
        content: vec![ContentBlock::Text(TextContent {
            text,
            text_signature: None,
        })],
        // The shell accepts only the source/category marker and reconstructs
        // retryability, fallback guidance, and the fixed safe message itself.
        details: Some(details),
        is_error: true,
    }
}

pub(super) fn classify_input_failure(message: &str) -> ToolFailureCategory {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("timed out")
        || lowered.contains("timeout")
        || lowered.contains("still in progress after")
    {
        ToolFailureCategory::Timeout
    } else if lowered.contains("index") && lowered.contains("fail") {
        ToolFailureCategory::IndexFailure
    } else if lowered.contains("not ready") {
        ToolFailureCategory::ProjectNotReady
    } else {
        ToolFailureCategory::InvalidModelInput
    }
}

pub(super) fn classify_mcp_error(error: &McpError) -> ToolFailureCategory {
    match error {
        McpError::Spawn { .. } => ToolFailureCategory::ConfigurationStartup,
        McpError::Io { .. } | McpError::Cancelled { .. } => ToolFailureCategory::Transport,
        McpError::Timeout { .. } => ToolFailureCategory::Timeout,
        McpError::ProcessExited { .. } => ToolFailureCategory::ProcessExit,
        McpError::Json { operation, .. } if *operation == "encode request" => {
            ToolFailureCategory::InvalidModelInput
        }
        McpError::Rpc { message, .. } if explicitly_invalid_input(message) => {
            ToolFailureCategory::InvalidModelInput
        }
        McpError::ProtocolOverflow { direction, .. } if *direction == "outbound" => {
            ToolFailureCategory::InvalidModelInput
        }
        McpError::Json { .. }
        | McpError::Rpc { .. }
        | McpError::ProtocolOverflow { .. }
        | McpError::Protocol(_) => ToolFailureCategory::ProviderProtocol,
    }
}

fn explicitly_invalid_input(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("-32602")
        || lowered.contains("invalid input")
        || lowered.contains("invalid argument")
        || lowered.contains("invalid parameter")
        || lowered.contains("invalid params")
}

pub(super) fn classify_provider_failure(message: &str) -> ToolFailureCategory {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("timed out") || lowered.contains("timeout") {
        ToolFailureCategory::Timeout
    } else if lowered.contains("index") && (lowered.contains("fail") || lowered.contains("error")) {
        ToolFailureCategory::IndexFailure
    } else if lowered.contains("project")
        && (lowered.contains("not ready")
            || lowered.contains("not found")
            || lowered.contains("missing")
            || lowered.contains("unknown"))
    {
        ToolFailureCategory::ProjectNotReady
    } else if explicitly_invalid_input(message) {
        ToolFailureCategory::InvalidModelInput
    } else {
        ToolFailureCategory::ProviderProtocol
    }
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

/// Extracts a single unambiguous, allowlisted target from a targeted wrapper
/// call. Unknown fields, non-string values, duplicates, and incomplete values
/// deliberately produce no correlation rather than a lossy approximation.
pub(super) fn graph_correlation(public_name: &str, input: &Value) -> Option<GraphCorrelationV1> {
    let tool = GraphCorrelationToolV1::from_public_name(public_name)?;
    let candidates: &[(&str, GraphCorrelationTargetKindV1)] = match tool {
        GraphCorrelationToolV1::SearchGraph => &[
            ("query", GraphCorrelationTargetKindV1::GraphQuery),
            ("name_pattern", GraphCorrelationTargetKindV1::NamePattern),
            (
                "qn_pattern",
                GraphCorrelationTargetKindV1::QualifiedNamePattern,
            ),
        ],
        GraphCorrelationToolV1::SearchCode => &[("pattern", GraphCorrelationTargetKindV1::Pattern)],
        GraphCorrelationToolV1::TracePath => {
            &[("function_name", GraphCorrelationTargetKindV1::FunctionName)]
        }
        GraphCorrelationToolV1::GetCodeSnippet => &[(
            "qualified_name",
            GraphCorrelationTargetKindV1::QualifiedName,
        )],
    };
    let mut selected = None;
    for (field, target_kind) in candidates {
        let Some(value) = input.get(*field) else {
            continue;
        };
        let target = value.as_str()?;
        if selected.replace((*target_kind, target)).is_some() {
            return None;
        }
    }
    let (target_kind, target) = selected?;
    GraphCorrelationV1::new(tool, target_kind, target)
}
