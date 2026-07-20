use std::panic::{AssertUnwindSafe, catch_unwind};

use temper_protocol_activity::{
    AgentRunEventV1, ModelCallStatusV1, RunStatusV1, ScopeStatusV1, StopReasonV1, ToolStatusV1,
    UsageV1,
};

use super::{ActivitySpanAttributes, ActivitySpanStatus};

pub(super) fn safe_export(operation: impl FnOnce()) {
    let _ = catch_unwind(AssertUnwindSafe(operation));
}

pub(super) fn scoped_attributes(event: &AgentRunEventV1) -> ActivitySpanAttributes {
    ActivitySpanAttributes {
        scope_id: Some(event.scope.id.clone()),
        parent_scope_id: event.scope.parent_id.clone(),
        turn: event.turn,
        ..ActivitySpanAttributes::default()
    }
}

pub(super) fn add_usage(total: &mut UsageV1, usage: &UsageV1) {
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.cache_read_tokens = total
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    total.cache_write_tokens = total
        .cache_write_tokens
        .saturating_add(usage.cache_write_tokens);
}

pub(super) fn run_span_id(event: &AgentRunEventV1) -> String {
    format!("{}:run", event.run_id)
}

pub(super) fn scope_span_id(event: &AgentRunEventV1) -> String {
    format!("{}:scope:{}", event.run_id, event.scope.id)
}

pub(super) fn scope_parent_span_id(event: &AgentRunEventV1) -> Option<String> {
    Some(event.scope.parent_id.as_ref().map_or_else(
        || run_span_id(event),
        |parent| format!("{}:scope:{parent}", event.run_id),
    ))
}

pub(super) fn turn_span_id(event: &AgentRunEventV1) -> String {
    format!(
        "{}:scope:{}:turn:{}",
        event.run_id,
        event.scope.id,
        event.turn.unwrap_or_default()
    )
}

pub(super) fn model_span_id(event: &AgentRunEventV1, call_id: &str, attempt: u32) -> String {
    format!("{}:model:{call_id}:{attempt}", event.run_id)
}

pub(super) fn tool_span_id(event: &AgentRunEventV1, call_id: &str) -> String {
    format!("{}:tool:{call_id}", event.run_id)
}

pub(super) fn operation_parent_id(event: &AgentRunEventV1) -> String {
    if event.turn.is_some() {
        turn_span_id(event)
    } else {
        scope_span_id(event)
    }
}

pub(super) fn stop_reason(reason: StopReasonV1) -> String {
    match reason {
        StopReasonV1::EndTurn => "end_turn",
        StopReasonV1::ToolUse => "tool_use",
        StopReasonV1::MaxTokens => "max_tokens",
        StopReasonV1::Cancelled => "cancelled",
        StopReasonV1::Error => "error",
    }
    .to_string()
}

pub(super) fn stop_status(reason: StopReasonV1) -> ActivitySpanStatus {
    match reason {
        StopReasonV1::Error => ActivitySpanStatus::Error,
        StopReasonV1::Cancelled => ActivitySpanStatus::Cancelled,
        _ => ActivitySpanStatus::Ok,
    }
}

pub(super) fn run_status(status: RunStatusV1) -> ActivitySpanStatus {
    match status {
        RunStatusV1::Succeeded => ActivitySpanStatus::Ok,
        RunStatusV1::Cancelled => ActivitySpanStatus::Cancelled,
    }
}

pub(super) fn scope_status(status: ScopeStatusV1) -> ActivitySpanStatus {
    match status {
        ScopeStatusV1::Succeeded => ActivitySpanStatus::Ok,
        ScopeStatusV1::Failed => ActivitySpanStatus::Error,
        ScopeStatusV1::Cancelled => ActivitySpanStatus::Cancelled,
    }
}

pub(super) fn model_status(
    status: ModelCallStatusV1,
    stop_reason: Option<StopReasonV1>,
) -> ActivitySpanStatus {
    if stop_reason == Some(StopReasonV1::Error) {
        return ActivitySpanStatus::Error;
    }
    match status {
        ModelCallStatusV1::Succeeded => ActivitySpanStatus::Ok,
        ModelCallStatusV1::Failed => ActivitySpanStatus::Error,
        ModelCallStatusV1::Cancelled => ActivitySpanStatus::Cancelled,
    }
}

pub(super) fn tool_status(status: ToolStatusV1) -> ActivitySpanStatus {
    match status {
        ToolStatusV1::Succeeded => ActivitySpanStatus::Ok,
        ToolStatusV1::Failed => ActivitySpanStatus::Error,
        ToolStatusV1::Cancelled => ActivitySpanStatus::Cancelled,
    }
}
