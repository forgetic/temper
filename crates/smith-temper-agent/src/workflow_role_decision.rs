//! Workflow-role decision responder for Temper's process protocol.
//!
//! Smith owns the concrete LLM call. Temper owns workflow authority: this module
//! reads a [`WorkflowRoleDecisionRequest`], asks the provider for one manifest
//! action, and returns a [`WorkflowRoleDecisionReply`]. Smith does not receive or
//! execute Forge/workflow mutation tools; it only chooses `no_action` or one of
//! the request's authorized action names.

use std::time::Instant;

use serde::Deserialize;
use temper_process_protocol::{
    BoundExternalTool, WORKFLOW_ROLE_DECISION_NO_ACTION, WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION,
    WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest, WorkflowRoleManifest,
};

use crate::decision::{DecisionError, run_decision};
use crate::observability::{REASON_PREVIEW_CHARS, redacted_preview};
use crate::provider::{ProviderConfig, ProviderError};
use crate::workflow_role_decision_capture::{
    CaptureWriteResult, WorkflowRoleDecisionCapture, WorkflowRoleDecisionCaptureInput,
};
use crate::workflow_role_decision_observability::{
    ProviderCallLogOutcome, ReplyLogMetadata, WorkflowRoleTrace, capture_write_failed_event,
    capture_written_event, emit, provider_call_finish_event, provider_call_start_event,
    reply_event, request_event,
};

const EXTERNAL_TOOL_SECTION: &str = "User-declared external tools";
const CODING_WORKSPACE_TOOL_ID: &str = "coding_workspace";

/// Provider-backed workflow-role decision responder.
pub struct WorkflowRoleDecisionResponder {
    provider: ProviderConfig,
    capture: WorkflowRoleDecisionCapture,
}

impl WorkflowRoleDecisionResponder {
    /// Builds a responder using Smith's provider config.
    pub fn new(provider: ProviderConfig) -> Self {
        Self {
            provider,
            capture: WorkflowRoleDecisionCapture::from_env(),
        }
    }

    /// Runs one LLM-backed workflow-role decision.
    pub async fn respond(
        &self,
        request: &WorkflowRoleDecisionRequest,
    ) -> Result<WorkflowRoleDecisionReply, WorkflowRoleDecisionError> {
        let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);
        let system_prompt = workflow_role_system_prompt(request);
        let user_context = match workflow_role_user_context(request) {
            Ok(context) => context,
            Err(error) => {
                self.write_capture(DecisionCaptureArgs {
                    request,
                    trace: &trace,
                    system_prompt: Some(&system_prompt),
                    user_context: None,
                    model_decision: None,
                    final_reply: None,
                    latency_ms: None,
                    outcome: "request_context_error",
                    failure_class: Some("request_context_serialization"),
                });
                return Err(WorkflowRoleDecisionError::RequestContext(error));
            }
        };

        if let Err(error) = validate_request_version(request) {
            self.write_capture(DecisionCaptureArgs {
                request,
                trace: &trace,
                system_prompt: Some(&system_prompt),
                user_context: Some(&user_context),
                model_decision: None,
                final_reply: None,
                latency_ms: None,
                outcome: "unsupported_protocol_version",
                failure_class: Some("unsupported_protocol_version"),
            });
            return Err(error);
        }

        emit(request_event(
            request,
            &trace,
            &self.provider,
            system_prompt.chars().count(),
            user_context.chars().count(),
        ));
        emit(provider_call_start_event(request, &trace, &self.provider));

        let provider_call_started = Instant::now();
        let decision_result = run_decision::<WorkflowRoleModelDecision>(
            &self.provider,
            &system_prompt,
            &user_context,
        )
        .await;
        let latency_ms = elapsed_ms(provider_call_started);

        match decision_result {
            Ok(decision) => {
                let model_action = decision.action.trim().to_string();
                emit(provider_call_finish_event(
                    request,
                    &trace,
                    &self.provider,
                    latency_ms,
                    ProviderCallLogOutcome::Model {
                        action: &model_action,
                    },
                ));
                let model_decision = decision.clone();
                let validated = validated_reply_for_model_decision(request, decision);
                emit(reply_event(
                    request,
                    &trace,
                    &self.provider,
                    &validated.reply,
                    &validated.log_metadata,
                ));
                self.write_capture(DecisionCaptureArgs {
                    request,
                    trace: &trace,
                    system_prompt: Some(&system_prompt),
                    user_context: Some(&user_context),
                    model_decision: Some(&model_decision),
                    final_reply: Some(&validated.reply),
                    latency_ms: Some(latency_ms),
                    outcome: validated.log_metadata.outcome,
                    failure_class: None,
                });
                Ok(validated.reply)
            }
            Err(DecisionError::Provider(error)) => {
                let error = DecisionError::Provider(error);
                emit(provider_call_finish_event(
                    request,
                    &trace,
                    &self.provider,
                    latency_ms,
                    ProviderCallLogOutcome::Error(&error),
                ));
                self.write_capture(DecisionCaptureArgs {
                    request,
                    trace: &trace,
                    system_prompt: Some(&system_prompt),
                    user_context: Some(&user_context),
                    model_decision: None,
                    final_reply: None,
                    latency_ms: Some(latency_ms),
                    outcome: "provider_error",
                    failure_class: Some(decision_failure_class(&error)),
                });
                Err(WorkflowRoleDecisionError::Decision(error))
            }
            Err(error) => {
                emit(provider_call_finish_event(
                    request,
                    &trace,
                    &self.provider,
                    latency_ms,
                    ProviderCallLogOutcome::Error(&error),
                ));
                let reply = no_action_for_request(request, "decision failed");
                let log_metadata = ReplyLogMetadata::decision_error_no_action();
                emit(reply_event(
                    request,
                    &trace,
                    &self.provider,
                    &reply,
                    &log_metadata,
                ));
                self.write_capture(DecisionCaptureArgs {
                    request,
                    trace: &trace,
                    system_prompt: Some(&system_prompt),
                    user_context: Some(&user_context),
                    model_decision: None,
                    final_reply: Some(&reply),
                    latency_ms: Some(latency_ms),
                    outcome: log_metadata.outcome,
                    failure_class: Some(decision_failure_class(&error)),
                });
                Ok(reply)
            }
        }
    }

    fn write_capture(&self, args: DecisionCaptureArgs<'_>) {
        let result = self.capture.write(WorkflowRoleDecisionCaptureInput {
            request: args.request,
            trace: args.trace,
            provider: &self.provider,
            system_prompt: args.system_prompt,
            user_context: args.user_context,
            model_decision: args.model_decision,
            final_reply: args.final_reply,
            latency_ms: args.latency_ms,
            outcome: args.outcome,
            failure_class: args.failure_class,
        });

        match result {
            CaptureWriteResult::Disabled => {}
            CaptureWriteResult::Written(path) => emit(capture_written_event(
                args.request,
                args.trace,
                &self.provider,
                &path,
            )),
            CaptureWriteResult::Failed(error) => emit(capture_write_failed_event(
                args.request,
                args.trace,
                &self.provider,
                error.class(),
                error.message(),
            )),
        }
    }
}

struct DecisionCaptureArgs<'a> {
    request: &'a WorkflowRoleDecisionRequest,
    trace: &'a WorkflowRoleTrace,
    system_prompt: Option<&'a str>,
    user_context: Option<&'a str>,
    model_decision: Option<&'a WorkflowRoleModelDecision>,
    final_reply: Option<&'a WorkflowRoleDecisionReply>,
    latency_ms: Option<u64>,
    outcome: &'static str,
    failure_class: Option<&'static str>,
}

/// Minimal model decision shape. Extra fields are ignored for compatibility with
/// older prompts that returned diagnostics beyond `action` and `reason`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct WorkflowRoleModelDecision {
    /// One manifest action, or `no_action`.
    pub action: String,
    /// Short rationale for operator/debug logs.
    #[serde(default)]
    pub reason: String,
}

impl WorkflowRoleModelDecision {
    /// Builds a safe no-action model decision, mostly for tests.
    pub fn no_action(reason: impl Into<String>) -> Self {
        Self {
            action: WORKFLOW_ROLE_DECISION_NO_ACTION.to_string(),
            reason: reason.into(),
        }
    }

    /// Builds a model decision choosing an action.
    pub fn action(action: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            reason: reason.into(),
        }
    }
}

/// Workflow-role responder failure.
#[derive(Debug)]
pub enum WorkflowRoleDecisionError {
    /// The request uses a protocol version this Smith binary does not implement.
    UnsupportedProtocolVersion { actual: u32 },
    /// Building the provider or obtaining a model decision failed in a way that
    /// should fail the worker rather than silently no-op.
    Decision(DecisionError),
    /// Smith could not serialize the model context for the provider call.
    RequestContext(serde_json::Error),
}

impl std::fmt::Display for WorkflowRoleDecisionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProtocolVersion { actual } => write!(
                formatter,
                "unsupported workflow-role decision protocol version {actual}; expected {WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION}"
            ),
            Self::Decision(error) => write!(formatter, "{error}"),
            Self::RequestContext(error) => {
                write!(
                    formatter,
                    "serializing workflow-role decision context failed: {error}"
                )
            }
        }
    }
}

impl std::error::Error for WorkflowRoleDecisionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decision(error) => Some(error),
            Self::RequestContext(error) => Some(error),
            Self::UnsupportedProtocolVersion { .. } => None,
        }
    }
}

fn validate_request_version(
    request: &WorkflowRoleDecisionRequest,
) -> Result<(), WorkflowRoleDecisionError> {
    if request.protocol_version == WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(WorkflowRoleDecisionError::UnsupportedProtocolVersion {
            actual: request.protocol_version,
        })
    }
}

fn decision_failure_class(error: &DecisionError) -> &'static str {
    match error {
        DecisionError::Provider(ProviderError::KeyUnavailable(_)) => "api_key_unavailable",
        DecisionError::Provider(ProviderError::OAuthUnavailable(_)) => "chatgpt_oauth_unavailable",
        DecisionError::Provider(ProviderError::AnthropicOAuthUnavailable(_)) => {
            "anthropic_oauth_unavailable"
        }
        DecisionError::Provider(ProviderError::Build(_)) => "provider_build",
        DecisionError::Run(_) => "provider_run",
        DecisionError::Empty => "empty_response",
        DecisionError::Parse { .. } => "json_parse",
    }
}

/// Builds the generated runtime system prompt for a workflow-role request.
pub fn workflow_role_system_prompt(request: &WorkflowRoleDecisionRequest) -> String {
    runtime_system_prompt(&request.role_manifest, &request.available_external_tools)
}

fn runtime_system_prompt(manifest: &WorkflowRoleManifest, tools: &[BoundExternalTool]) -> String {
    if manifest.external_tools.is_empty() {
        return manifest.prompt.render();
    }
    let mut prompt = manifest.prompt.clone();
    if let Some(section) = prompt.section_mut(EXTERNAL_TOOL_SECTION) {
        section.lines = runtime_external_tool_lines(tools);
    }
    prompt.render()
}

/// Builds the user-context JSON string sent to the provider.
pub fn workflow_role_user_context(
    request: &WorkflowRoleDecisionRequest,
) -> Result<String, serde_json::Error> {
    let allowed_actions = std::iter::once(WORKFLOW_ROLE_DECISION_NO_ACTION.to_string())
        .chain(
            request
                .authorized_actions
                .iter()
                .map(|action| action.action.clone()),
        )
        .collect::<Vec<_>>();
    let context = serde_json::json!({
        "work_item": request.work_item_context,
        "allowed_actions": allowed_actions,
        "authorized_actions": request.authorized_actions,
        "available_external_tools": request.available_external_tools,
    });
    serde_json::to_string_pretty(&context)
}

/// Validates a model decision and turns unauthorized actions into `no_action`.
pub fn reply_for_model_decision(
    request: &WorkflowRoleDecisionRequest,
    decision: WorkflowRoleModelDecision,
) -> WorkflowRoleDecisionReply {
    validated_reply_for_model_decision(request, decision).reply
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ValidatedWorkflowRoleReply {
    reply: WorkflowRoleDecisionReply,
    log_metadata: ReplyLogMetadata,
}

fn validated_reply_for_model_decision(
    request: &WorkflowRoleDecisionRequest,
    decision: WorkflowRoleModelDecision,
) -> ValidatedWorkflowRoleReply {
    let action = decision.action.trim().to_string();
    if action == WORKFLOW_ROLE_DECISION_NO_ACTION {
        return ValidatedWorkflowRoleReply {
            reply: no_action_for_request(request, decision.reason),
            log_metadata: ReplyLogMetadata::no_action(action),
        };
    }
    if request
        .authorized_actions
        .iter()
        .any(|candidate| candidate.action == action)
    {
        return ValidatedWorkflowRoleReply {
            reply: WorkflowRoleDecisionReply {
                protocol_version: request.protocol_version,
                action: action.clone(),
                reason: decision.reason,
            },
            log_metadata: ReplyLogMetadata::authorized_action(action),
        };
    }

    ValidatedWorkflowRoleReply {
        reply: no_action_for_request(
            request,
            format!(
                "unauthorized model action: {}",
                redacted_preview(&action, REASON_PREVIEW_CHARS)
            ),
        ),
        log_metadata: ReplyLogMetadata::unauthorized_action_downgraded(action),
    }
}

fn no_action_for_request(
    request: &WorkflowRoleDecisionRequest,
    reason: impl Into<String>,
) -> WorkflowRoleDecisionReply {
    WorkflowRoleDecisionReply {
        protocol_version: request.protocol_version,
        action: WORKFLOW_ROLE_DECISION_NO_ACTION.to_string(),
        reason: reason.into(),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn runtime_external_tool_lines(tools: &[BoundExternalTool]) -> Vec<String> {
    let mut lines = vec![
        "Only the external tools listed in this section are bound and available for this run."
            .to_string(),
        "Declared tools not listed here are unavailable; do not claim to use them.".to_string(),
        "You do not and cannot call these tools yourself: selecting the workflow action a bound tool backs makes the engine run that tool automatically while it executes the action.".to_string(),
        "Because a bound tool runs on action selection, never return no_action just because you cannot run the tool directly or because its output (a branch, head, diff, or verdict) does not exist yet; selecting the action is what produces it.".to_string(),
        "External tools do not grant workflow or Forge mutation authority beyond the authorized workflow actions above.".to_string(),
    ];
    if tools.is_empty() {
        lines.push("(no external tools are bound for this run)".to_string());
    } else {
        for tool in tools {
            lines.push(format!(
                "{} via {}: {}",
                tool.id, tool.provider, tool.description
            ));
            if !tool.constraints.is_empty() {
                lines.push(format!(
                    "{} constraints: {}",
                    tool.id,
                    tool.constraints.join("; ")
                ));
            }
            if tool.id == CODING_WORKSPACE_TOOL_ID {
                lines.push(format!(
                    "{} rule: it is bound, so selecting the PR-opening workflow action runs it to produce the implementation branch/head and then opens the PR. Choose that PR-opening action for ready code work; do not return no_action expecting to run the workspace first.",
                    tool.id
                ));
            }
            if let Some(guidance) = &tool.guidance {
                lines.push(format!("{} guidance: {guidance}", tool.id));
            }
        }
    }
    lines
}

#[cfg(test)]
#[path = "workflow_role_decision_tests.rs"]
mod tests;
