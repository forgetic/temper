use super::*;

// This is a closed provider outcome, not text to search within an arbitrary
// failure. Near-matches remain provider/protocol failures.
const EXPLORATION_CLOSED_PROVIDER_OUTCOME: &str = "exploration_closed";

pub(in crate::codebase_memory) fn classify_input_failure(message: &str) -> ToolFailureCategory {
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

pub(in crate::codebase_memory) fn classify_mcp_error(error: &McpError) -> ToolFailureCategory {
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

pub(in crate::codebase_memory) fn classify_provider_failure(message: &str) -> ToolFailureCategory {
    if message == EXPLORATION_CLOSED_PROVIDER_OUTCOME {
        return ToolFailureCategory::GraphLifecycleDenial;
    }
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
    } else if explicitly_invalid_input(message) || candidate_lookup_miss(message) {
        ToolFailureCategory::InvalidModelInput
    } else {
        ToolFailureCategory::ProviderProtocol
    }
}

fn candidate_lookup_miss(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    let names_candidate = [
        "candidate",
        "function",
        "qualified_name",
        "selector",
        "source",
        "symbol",
    ]
    .into_iter()
    .any(|fragment| lowered.contains(fragment));
    let reports_miss = [
        "does not exist",
        "missing",
        "no function",
        "no match",
        "not found",
        "unknown",
        "unavailable",
    ]
    .into_iter()
    .any(|fragment| lowered.contains(fragment));
    names_candidate && reports_miss
}
