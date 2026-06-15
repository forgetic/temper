//! Workflow-role decision event construction and trace extraction.

use serde_json::Value;
use temper_process_protocol::{
    WORKFLOW_ROLE_DECISION_NO_ACTION, WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
};

use crate::decision::DecisionError;
use crate::observability::{
    FIELD_PREVIEW_CHARS, REASON_PREVIEW_CHARS, StructuredEvent, redacted_preview, scalar_preview,
};
use crate::provider::{ProviderConfig, ProviderError};

/// Authority-neutral identifiers Temper may place in the work-item context.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WorkflowRoleTrace {
    pub(crate) run_id: Option<String>,
    pub(crate) tick_id: Option<String>,
    pub(crate) work_item_id: Option<String>,
    pub(crate) decision_id: Option<String>,
    pub(crate) repository: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) queue: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) artifact_type: Option<String>,
    pub(crate) artifact_number: Option<String>,
}

impl WorkflowRoleTrace {
    /// Extracts known scalar fields, ignoring missing or non-scalar values.
    pub(crate) fn from_work_item_context(context: &Value) -> Self {
        Self {
            run_id: nested_scalar(context, &["observability", "run_id"]),
            tick_id: nested_scalar(context, &["observability", "tick_id"]),
            work_item_id: nested_scalar(context, &["observability", "work_item_id"]),
            decision_id: nested_scalar(context, &["observability", "decision_id"]),
            repository: nested_scalar(context, &["repository"])
                .or_else(|| nested_scalar(context, &["observability", "repo"])),
            role: nested_scalar(context, &["role"])
                .or_else(|| nested_scalar(context, &["observability", "role"])),
            queue: nested_scalar(context, &["queue"])
                .or_else(|| nested_scalar(context, &["observability", "queue"])),
            kind: nested_scalar(context, &["kind"])
                .or_else(|| nested_scalar(context, &["observability", "artifact_kind"])),
            artifact_type: nested_scalar(context, &["artifact", "type"])
                .or_else(|| nested_scalar(context, &["observability", "artifact_type"])),
            artifact_number: nested_scalar(context, &["artifact", "number"])
                .or_else(|| nested_scalar(context, &["observability", "artifact_number"])),
        }
    }
}

/// Reply metadata that anvil logs after validating the model action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReplyLogMetadata {
    pub(crate) model_action: Option<String>,
    pub(crate) unauthorized_model_action: Option<String>,
    pub(crate) outcome: &'static str,
}

impl ReplyLogMetadata {
    pub(crate) fn authorized_action(model_action: String) -> Self {
        Self {
            model_action: Some(model_action),
            unauthorized_model_action: None,
            outcome: "authorized_action",
        }
    }

    pub(crate) fn no_action(model_action: String) -> Self {
        Self {
            model_action: Some(model_action),
            unauthorized_model_action: None,
            outcome: "no_action",
        }
    }

    pub(crate) fn unauthorized_action_downgraded(model_action: String) -> Self {
        Self {
            model_action: Some(model_action.clone()),
            unauthorized_model_action: Some(model_action),
            outcome: "unauthorized_action_downgraded",
        }
    }

    pub(crate) fn decision_error_no_action() -> Self {
        Self {
            model_action: None,
            unauthorized_model_action: None,
            outcome: "decision_error_no_action",
        }
    }
}

/// Provider-call finish outcome.
pub(crate) enum ProviderCallLogOutcome<'a> {
    /// The model returned a parseable action.
    Model { action: &'a str },
    /// anvil failed before or during parsing.
    Error(&'a DecisionError),
}

pub(crate) fn request_event(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
    prompt_chars: usize,
    context_chars: usize,
) -> StructuredEvent {
    let allowed_actions = allowed_action_names(request);
    let external_tools = external_tool_ids(request);
    with_provider_fields(
        base_event("anvil.workflow_role_decision.request", request, trace),
        provider,
    )
    .strings("allowed_actions", &allowed_actions)
    .usize("allowed_action_count", allowed_actions.len())
    .strings("available_external_tools", &external_tools)
    .usize("available_external_tool_count", external_tools.len())
    .usize("prompt_chars", prompt_chars)
    .usize("context_chars", context_chars)
}

pub(crate) fn provider_call_start_event(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
) -> StructuredEvent {
    with_provider_fields(
        base_event(
            "anvil.workflow_role_decision.provider_call.start",
            request,
            trace,
        ),
        provider,
    )
}

pub(crate) fn provider_call_finish_event(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
    latency_ms: u64,
    outcome: ProviderCallLogOutcome<'_>,
) -> StructuredEvent {
    let event = with_provider_fields(
        base_event(
            "anvil.workflow_role_decision.provider_call.finish",
            request,
            trace,
        ),
        provider,
    )
    .u64("latency_ms", latency_ms);

    match outcome {
        ProviderCallLogOutcome::Model { action } => {
            event.str("outcome", "ok").str("model_action", action)
        }
        ProviderCallLogOutcome::Error(error) => event
            .str("outcome", "error")
            .opt_str("provider_error_class", provider_error_class(error))
            .opt_str("parse_error_class", parse_error_class(error)),
    }
}

pub(crate) fn reply_event(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
    reply: &WorkflowRoleDecisionReply,
    metadata: &ReplyLogMetadata,
) -> StructuredEvent {
    with_provider_fields(
        base_event("anvil.workflow_role_decision.reply", request, trace),
        provider,
    )
    .str("outcome", metadata.outcome)
    .opt_str("model_action", metadata.model_action.as_deref())
    .str("returned_action", &reply.action)
    .bool(
        "unauthorized_action_downgraded",
        metadata.unauthorized_model_action.is_some(),
    )
    .opt_str(
        "unauthorized_model_action",
        metadata.unauthorized_model_action.as_deref(),
    )
    .str(
        "reason_preview",
        redacted_preview(&reply.reason, REASON_PREVIEW_CHARS),
    )
}

pub(crate) fn capture_written_event(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
    path: &std::path::Path,
) -> StructuredEvent {
    with_provider_fields(
        base_event(
            "anvil.workflow_role_decision.capture.written",
            request,
            trace,
        ),
        provider,
    )
    .str("capture_path", path.display().to_string())
}

pub(crate) fn capture_write_failed_event(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
    error_class: &str,
    error_message: &str,
) -> StructuredEvent {
    with_provider_fields(
        base_event(
            "anvil.workflow_role_decision.capture.write_failed",
            request,
            trace,
        ),
        provider,
    )
    .str("outcome", "warning")
    .str("capture_error_class", error_class)
    .str(
        "capture_error_preview",
        redacted_preview(error_message, FIELD_PREVIEW_CHARS),
    )
}

pub(crate) fn emit(event: StructuredEvent) {
    eprintln!("{}", event.render());
}

fn nested_scalar(context: &Value, path: &[&str]) -> Option<String> {
    let mut current = context;
    for segment in path {
        current = current.get(*segment)?;
    }
    scalar_preview(Some(current))
}

fn base_event(
    event: &str,
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
) -> StructuredEvent {
    StructuredEvent::new(event)
        .str("workflow_id", &request.workflow_id)
        .str("role", request.role_manifest.id.as_str())
        .opt_str("work_item_role", trace.role.as_deref())
        .opt_str("run_id", trace.run_id.as_deref())
        .opt_str("tick_id", trace.tick_id.as_deref())
        .opt_str("work_item_id", trace.work_item_id.as_deref())
        .opt_str("decision_id", trace.decision_id.as_deref())
        .opt_str("repository", trace.repository.as_deref())
        .opt_str("queue", trace.queue.as_deref())
        .opt_str("kind", trace.kind.as_deref())
        .opt_str("artifact_type", trace.artifact_type.as_deref())
        .opt_str("artifact_number", trace.artifact_number.as_deref())
}

fn with_provider_fields(event: StructuredEvent, provider: &ProviderConfig) -> StructuredEvent {
    let identity = provider.observability_identity();
    event
        .str("provider", identity.provider_id)
        .str("model", identity.model_id)
        .str("auth_mode", identity.auth_mode)
}

fn allowed_action_names(request: &WorkflowRoleDecisionRequest) -> Vec<String> {
    std::iter::once(WORKFLOW_ROLE_DECISION_NO_ACTION.to_string())
        .chain(
            request
                .authorized_actions
                .iter()
                .map(|action| action.action.clone()),
        )
        .collect()
}

fn external_tool_ids(request: &WorkflowRoleDecisionRequest) -> Vec<String> {
    request
        .available_external_tools
        .iter()
        .map(|tool| tool.id.as_str().to_string())
        .collect()
}

fn provider_error_class(error: &DecisionError) -> Option<&'static str> {
    match error {
        DecisionError::Provider(ProviderError::KeyUnavailable(_)) => Some("api_key_unavailable"),
        DecisionError::Provider(ProviderError::OAuthUnavailable(_)) => {
            Some("chatgpt_oauth_unavailable")
        }
        DecisionError::Provider(ProviderError::AnthropicOAuthUnavailable(_)) => {
            Some("anthropic_oauth_unavailable")
        }
        DecisionError::Provider(ProviderError::Build(_)) => Some("provider_build"),
        DecisionError::Run(_) => Some("provider_run"),
        DecisionError::Empty | DecisionError::Parse { .. } => None,
    }
}

fn parse_error_class(error: &DecisionError) -> Option<&'static str> {
    match error {
        DecisionError::Empty => Some("empty_response"),
        DecisionError::Parse { .. } => Some("json_parse"),
        DecisionError::Provider(_) | DecisionError::Run(_) => None,
    }
}

#[cfg(test)]
#[path = "workflow_role_decision_observability_tests.rs"]
mod tests;
