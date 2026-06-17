//! Workflow-role decision observability: real-field `tracing` emission and
//! trace extraction.
//!
//! Each workflow-role decision emits a handful of **between-state-cause** lines —
//! the provider request/start/finish, the resolved reply, and the optional
//! capture-file outcome. Per the level policy in
//! `docs/explanation/logging-and-observability.md` §5 these are *causes between*
//! the engine's authoritative workflow-state changes (the §3 catalog), not state
//! changes themselves, so they sit at **debug** (the failed-capture warning is the
//! one exception, emitted at `warn`). They carry **real `tracing` fields** on
//! `target: "temper::agent"` with the `::` namespace `RUST_LOG`/§2 key off — never
//! a JSON string crammed into the message body.
//!
//! The `event=` values are plain, dot-namespaced strings (e.g.
//! `anvil.workflow_role_decision.reply`); they deliberately sit **off** the closed
//! `temper-log` `Event` catalog (the info contract) — this crate must not depend
//! on `temper-log` (depgraph tier) and these lines are not state changes.
//!
//! All free-form model/operator text is bounded and redacted via the local
//! `observability::{redacted_preview, scalar_preview, preview}` helpers before it
//! reaches a field.

use serde_json::Value;
use temper_protocol_decision::{
    WORKFLOW_ROLE_DECISION_NO_ACTION, WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
};

use crate::decision::DecisionError;
use crate::observability::{
    FIELD_PREVIEW_CHARS, REASON_PREVIEW_CHARS, preview, redacted_preview, scalar_preview,
};
use crate::provider::{ProviderConfig, ProviderError};

/// The tracing target every workflow-role decision line is emitted on. The `::`
/// namespace (not the legacy `temper_agent` underscore) is what `RUST_LOG` and
/// the §2 service map key off.
const AGENT_TARGET: &str = "temper::agent";

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

/// The bounded identity/correlation fields common to every decision line,
/// extracted once from the request, trace, and provider so each `tracing` macro
/// call records the same vocabulary.
struct DecisionFields {
    workflow_id: String,
    role: String,
    work_item_role: String,
    run_id: String,
    tick_id: String,
    work_item_id: String,
    decision_id: String,
    repository: String,
    queue: String,
    kind: String,
    artifact_type: String,
    artifact_number: String,
    provider: String,
    model: String,
    auth_mode: String,
}

impl DecisionFields {
    fn new(
        request: &WorkflowRoleDecisionRequest,
        trace: &WorkflowRoleTrace,
        provider: &ProviderConfig,
    ) -> Self {
        let identity = provider.observability_identity();
        Self {
            workflow_id: bounded(&request.workflow_id),
            role: bounded(request.role_manifest.id.as_str()),
            work_item_role: opt(trace.role.as_deref()),
            run_id: opt(trace.run_id.as_deref()),
            tick_id: opt(trace.tick_id.as_deref()),
            work_item_id: opt(trace.work_item_id.as_deref()),
            decision_id: opt(trace.decision_id.as_deref()),
            repository: opt(trace.repository.as_deref()),
            queue: opt(trace.queue.as_deref()),
            kind: opt(trace.kind.as_deref()),
            artifact_type: opt(trace.artifact_type.as_deref()),
            artifact_number: opt(trace.artifact_number.as_deref()),
            provider: bounded(identity.provider_id),
            model: bounded(identity.model_id),
            auth_mode: bounded(identity.auth_mode),
        }
    }

    /// A short `[repo#n role]`-ish correlation tag for the human message body.
    fn tag(&self) -> String {
        let subject = if self.work_item_id.is_empty() {
            self.workflow_id.as_str()
        } else {
            self.work_item_id.as_str()
        };
        format!("[{subject} {}]", self.role)
    }
}

/// Emits the `decision.request` line at debug: provider identity plus the
/// allowed-action and external-tool counts/names and prompt/context sizes.
pub(crate) fn emit_request(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
    prompt_chars: usize,
    context_chars: usize,
) {
    let f = DecisionFields::new(request, trace, provider);
    let allowed_actions = bounded_list(allowed_action_names(request));
    let allowed_action_count = allowed_actions.len();
    let external_tools = bounded_list(external_tool_ids(request));
    let available_external_tool_count = external_tools.len();
    let tag = f.tag();
    tracing::debug!(
        target: AGENT_TARGET,
        event = "anvil.workflow_role_decision.request",
        workflow_id = %f.workflow_id,
        role = %f.role,
        work_item_role = %f.work_item_role,
        run_id = %f.run_id,
        tick_id = %f.tick_id,
        work_item_id = %f.work_item_id,
        decision_id = %f.decision_id,
        repository = %f.repository,
        queue = %f.queue,
        kind = %f.kind,
        artifact_type = %f.artifact_type,
        artifact_number = %f.artifact_number,
        provider = %f.provider,
        model = %f.model,
        auth_mode = %f.auth_mode,
        allowed_actions = ?allowed_actions,
        allowed_action_count,
        available_external_tools = ?external_tools,
        available_external_tool_count,
        prompt_chars,
        context_chars,
        "agent: {tag} decision request | provider={} model={} actions={allowed_action_count} tools={available_external_tool_count}",
        f.provider, f.model,
    );
}

/// Emits the `provider_call.start` line at debug: we are about to issue the LLM
/// call for this decision.
pub(crate) fn emit_provider_call_start(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
) {
    let f = DecisionFields::new(request, trace, provider);
    let tag = f.tag();
    tracing::debug!(
        target: AGENT_TARGET,
        event = "anvil.workflow_role_decision.provider_call.start",
        workflow_id = %f.workflow_id,
        role = %f.role,
        work_item_role = %f.work_item_role,
        run_id = %f.run_id,
        tick_id = %f.tick_id,
        work_item_id = %f.work_item_id,
        decision_id = %f.decision_id,
        repository = %f.repository,
        queue = %f.queue,
        kind = %f.kind,
        artifact_type = %f.artifact_type,
        artifact_number = %f.artifact_number,
        provider = %f.provider,
        model = %f.model,
        auth_mode = %f.auth_mode,
        "agent: {tag} provider call start | provider={} model={}",
        f.provider, f.model,
    );
}

/// Emits the `provider_call.finish` line at debug: the LLM call returned, with
/// the latency and either the parsed action or the redaction-safe error classes.
pub(crate) fn emit_provider_call_finish(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
    latency_ms: u64,
    outcome: ProviderCallLogOutcome<'_>,
) {
    let f = DecisionFields::new(request, trace, provider);
    let tag = f.tag();
    let (outcome_label, model_action, provider_error_class, parse_error_class) = match outcome {
        ProviderCallLogOutcome::Model { action } => {
            ("ok", bounded(action), String::new(), String::new())
        }
        ProviderCallLogOutcome::Error(error) => (
            "error",
            String::new(),
            opt(provider_error_class(error)),
            opt(parse_error_class(error)),
        ),
    };
    tracing::debug!(
        target: AGENT_TARGET,
        event = "anvil.workflow_role_decision.provider_call.finish",
        workflow_id = %f.workflow_id,
        role = %f.role,
        work_item_role = %f.work_item_role,
        run_id = %f.run_id,
        tick_id = %f.tick_id,
        work_item_id = %f.work_item_id,
        decision_id = %f.decision_id,
        repository = %f.repository,
        queue = %f.queue,
        kind = %f.kind,
        artifact_type = %f.artifact_type,
        artifact_number = %f.artifact_number,
        provider = %f.provider,
        model = %f.model,
        auth_mode = %f.auth_mode,
        latency_ms,
        outcome = %outcome_label,
        model_action = %model_action,
        provider_error_class = %provider_error_class,
        parse_error_class = %parse_error_class,
        "agent: {tag} provider call finish | outcome={outcome_label} latency_ms={latency_ms}",
    );
}

/// Emits the `decision.reply` line at debug: the validated reply this decision
/// resolved to, including any unauthorized-action downgrade and the redacted
/// reason preview.
pub(crate) fn emit_reply(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
    reply: &WorkflowRoleDecisionReply,
    metadata: &ReplyLogMetadata,
) {
    let f = DecisionFields::new(request, trace, provider);
    let tag = f.tag();
    let model_action = opt(metadata.model_action.as_deref());
    let returned_action = bounded(&reply.action);
    let unauthorized_action_downgraded = metadata.unauthorized_model_action.is_some();
    let unauthorized_model_action = opt(metadata.unauthorized_model_action.as_deref());
    let reason_preview = redacted_preview(&reply.reason, REASON_PREVIEW_CHARS);
    tracing::debug!(
        target: AGENT_TARGET,
        event = "anvil.workflow_role_decision.reply",
        workflow_id = %f.workflow_id,
        role = %f.role,
        work_item_role = %f.work_item_role,
        run_id = %f.run_id,
        tick_id = %f.tick_id,
        work_item_id = %f.work_item_id,
        decision_id = %f.decision_id,
        repository = %f.repository,
        queue = %f.queue,
        kind = %f.kind,
        artifact_type = %f.artifact_type,
        artifact_number = %f.artifact_number,
        provider = %f.provider,
        model = %f.model,
        auth_mode = %f.auth_mode,
        outcome = %metadata.outcome,
        model_action = %model_action,
        returned_action = %returned_action,
        unauthorized_action_downgraded,
        unauthorized_model_action = %unauthorized_model_action,
        reason_preview = %reason_preview,
        "agent: {tag} decision reply | outcome={} action={returned_action}",
        metadata.outcome,
    );
}

/// Emits the `capture.written` line at debug: the decision capture file was
/// persisted at `path`.
pub(crate) fn emit_capture_written(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
    path: &std::path::Path,
) {
    let f = DecisionFields::new(request, trace, provider);
    let tag = f.tag();
    let capture_path = bounded(&path.display().to_string());
    tracing::debug!(
        target: AGENT_TARGET,
        event = "anvil.workflow_role_decision.capture.written",
        workflow_id = %f.workflow_id,
        role = %f.role,
        work_item_role = %f.work_item_role,
        run_id = %f.run_id,
        tick_id = %f.tick_id,
        work_item_id = %f.work_item_id,
        decision_id = %f.decision_id,
        repository = %f.repository,
        queue = %f.queue,
        kind = %f.kind,
        artifact_type = %f.artifact_type,
        artifact_number = %f.artifact_number,
        provider = %f.provider,
        model = %f.model,
        auth_mode = %f.auth_mode,
        capture_path = %capture_path,
        "agent: {tag} decision capture written | {capture_path}",
    );
}

/// Emits the `capture.write_failed` line at **warn**: persisting the decision
/// capture failed. The error message is bounded and redacted before logging.
pub(crate) fn emit_capture_write_failed(
    request: &WorkflowRoleDecisionRequest,
    trace: &WorkflowRoleTrace,
    provider: &ProviderConfig,
    error_class: &str,
    error_message: &str,
) {
    let f = DecisionFields::new(request, trace, provider);
    let tag = f.tag();
    let capture_error_class = bounded(error_class);
    let capture_error_preview = redacted_preview(error_message, FIELD_PREVIEW_CHARS);
    tracing::warn!(
        target: AGENT_TARGET,
        event = "anvil.workflow_role_decision.capture.write_failed",
        workflow_id = %f.workflow_id,
        role = %f.role,
        work_item_role = %f.work_item_role,
        run_id = %f.run_id,
        tick_id = %f.tick_id,
        work_item_id = %f.work_item_id,
        decision_id = %f.decision_id,
        repository = %f.repository,
        queue = %f.queue,
        kind = %f.kind,
        artifact_type = %f.artifact_type,
        artifact_number = %f.artifact_number,
        provider = %f.provider,
        model = %f.model,
        auth_mode = %f.auth_mode,
        outcome = "warning",
        capture_error_class = %capture_error_class,
        capture_error_preview = %capture_error_preview,
        "agent: {tag} decision capture write failed | class={capture_error_class}",
    );
}

/// Bounds a single scalar field to one redaction-safe preview line. (These
/// fields are identity/config values, not free-form model text, so the bound is
/// the whitespace-collapsing `preview`, matching the prior `StructuredEvent`
/// `.str(..)` behavior.)
fn bounded(value: &str) -> String {
    preview(value, FIELD_PREVIEW_CHARS)
}

/// Bounds an optional scalar field, rendering `None` as the empty string (so the
/// recorded field is present but blank, mirroring `opt_str`'s "omit when absent"
/// without the dynamic-field machinery `tracing` macros lack).
fn opt(value: Option<&str>) -> String {
    value.map(bounded).unwrap_or_default()
}

/// Bounds every element of a string list for a `?`-recorded array field.
fn bounded_list(values: Vec<String>) -> Vec<String> {
    values.iter().map(|value| bounded(value)).collect()
}

fn nested_scalar(context: &Value, path: &[&str]) -> Option<String> {
    let mut current = context;
    for segment in path {
        current = current.get(*segment)?;
    }
    scalar_preview(Some(current))
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
