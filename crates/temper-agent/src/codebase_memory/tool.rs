use super::*;

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
        let scope = Arc::clone(&self.scope);
        let mcp_name = self.mcp_name.clone();
        let default_project_key = self.default_project_key;
        let budget = MonotonicCallBudget::new(self.call_timeout);
        let readiness_started = Instant::now();
        let input = match skein::runtime::spawn_blocking(move || {
            let remaining = budget.remaining().unwrap_or(Duration::ZERO);
            scope.prepare_tool_input(&mcp_name, default_project_key, input, remaining)
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
                emit_failed_mcp_tool_result(self, "", category, timings);
                return Ok(codebase_memory_failure_output_with_timings(
                    category, None, timings,
                ));
            }
        };
        let readiness_wait_ms = duration_ms(readiness_started.elapsed());

        if self.mcp_name == "list_projects" {
            return Ok(self.scope.list_projects_output());
        }

        let mcp_project = input
            .get("project")
            .or_else(|| input.get("repo"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let argument_preview = redacted_preview(&input.to_string(), 240);
        emit_mcp_tool_called(McpToolCalled {
            tool_name: &self.public_name,
            mcp_tool: &self.mcp_name,
            mcp_project: &mcp_project,
            repo_root: &self.scope.primary_root().display().to_string(),
            argument_preview: &argument_preview,
        });

        let Some(remaining) = budget.remaining() else {
            let timings = ToolCallTimings {
                readiness_wait_ms,
                graph_execution_ms: 0,
                duration_ms: budget.elapsed_ms(),
            };
            emit_failed_mcp_tool_result(self, &mcp_project, ToolFailureCategory::Timeout, timings);
            return Ok(codebase_memory_failure_output_with_timings(
                ToolFailureCategory::Timeout,
                None,
                timings,
            ));
        };
        // Leave a small completion margin so the typed result wins the generic
        // shell timeout instead of being dropped at an equal deadline.
        let execution_margin = (remaining / 20).min(Duration::from_millis(10));
        let execution_budget = remaining.saturating_sub(execution_margin);
        if execution_budget.is_zero() {
            let timings = ToolCallTimings {
                readiness_wait_ms,
                graph_execution_ms: 0,
                duration_ms: budget.elapsed_ms(),
            };
            emit_failed_mcp_tool_result(self, &mcp_project, ToolFailureCategory::Timeout, timings);
            return Ok(codebase_memory_failure_output_with_timings(
                ToolFailureCategory::Timeout,
                None,
                timings,
            ));
        }
        let graph_started = Instant::now();
        let result = match temper_agent_io::timeout(
            execution_budget,
            self.client
                .call_tool(&self.mcp_name, input, self.call_timeout),
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
                emit_failed_mcp_tool_result(self, &mcp_project, category, timings);
                return Ok(codebase_memory_failure_output_with_timings(
                    category, None, timings,
                ));
            }
            Err(_) => {
                let timings = ToolCallTimings {
                    readiness_wait_ms,
                    graph_execution_ms: duration_ms(graph_started.elapsed()),
                    duration_ms: budget.elapsed_ms(),
                };
                emit_failed_mcp_tool_result(
                    self,
                    &mcp_project,
                    ToolFailureCategory::Timeout,
                    timings,
                );
                return Ok(codebase_memory_failure_output_with_timings(
                    ToolFailureCategory::Timeout,
                    None,
                    timings,
                ));
            }
        };
        let timings = ToolCallTimings {
            readiness_wait_ms,
            graph_execution_ms: duration_ms(graph_started.elapsed()),
            duration_ms: budget.elapsed_ms(),
        };
        let bounded = bound_text(&result.text, MAX_CODEBASE_MEMORY_OUTPUT_BYTES);
        let result_preview = redacted_preview(&bounded.text, 240);
        emit_mcp_tool_result(McpToolResult {
            tool_name: &self.public_name,
            mcp_tool: &self.mcp_name,
            mcp_project: &mcp_project,
            is_error: result.is_error,
            truncated: bounded.truncated,
            result_preview: &result_preview,
            readiness_wait_ms: timings.readiness_wait_ms,
            graph_execution_ms: timings.graph_execution_ms,
            duration_ms: timings.duration_ms,
        });
        if result.is_error {
            let category = classify_provider_failure(&bounded.text);
            return Ok(codebase_memory_failure_output_with_timings(
                category,
                Some(bounded.text),
                timings,
            ));
        }
        Ok(ToolOutput {
            content: vec![ContentBlock::Text(TextContent {
                text: bounded.text,
                text_signature: None,
            })],
            details: Some(json!({
                "mcp_tool": self.mcp_name,
                "truncated": bounded.truncated,
                "workspace_scope": self.scope.details_json(),
                "timing": {
                    "readiness_wait_ms": timings.readiness_wait_ms,
                    "graph_execution_ms": timings.graph_execution_ms,
                    "duration_ms": timings.duration_ms,
                },
            })),
            is_error: result.is_error,
        })
    }
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
    });
}

#[cfg(test)]
pub(super) fn codebase_memory_failure_output(
    category: ToolFailureCategory,
    model_text: Option<String>,
) -> ToolOutput {
    codebase_memory_failure_output_with_timings(category, model_text, ToolCallTimings::default())
}

fn codebase_memory_failure_output_with_timings(
    category: ToolFailureCategory,
    model_text: Option<String>,
    timings: ToolCallTimings,
) -> ToolOutput {
    let diagnostic = ToolFailureDiagnostic::codebase_memory(category);
    let text = model_text.unwrap_or_else(|| {
        format!(
            "{}; use read, grep, find, or other conventional discovery instead",
            diagnostic.message
        )
    });
    ToolOutput {
        content: vec![ContentBlock::Text(TextContent {
            text,
            text_signature: None,
        })],
        // The shell accepts only this source/category marker and reconstructs
        // retryability, fallback guidance, and the fixed safe message itself.
        details: Some(json!({
            SAFE_TOOL_FAILURE_DETAIL_KEY: {
                "source": "codebase_memory",
                "category": category.as_str(),
            },
            "timing": {
                "readiness_wait_ms": timings.readiness_wait_ms,
                "graph_execution_ms": timings.graph_execution_ms,
                "duration_ms": timings.duration_ms,
            },
        })),
        is_error: true,
    }
}

pub(super) fn classify_input_failure(message: &str) -> ToolFailureCategory {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("index") && lowered.contains("fail") {
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
        McpError::Json { .. }
        | McpError::Rpc { .. }
        | McpError::ProtocolOverflow { .. }
        | McpError::Protocol(_) => ToolFailureCategory::ProviderProtocol,
    }
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
    } else if lowered.contains("invalid input")
        || lowered.contains("invalid argument")
        || lowered.contains("invalid parameter")
    {
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
