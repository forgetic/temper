//! Workflow-role decision responder for Temper's process protocol.
//!
//! anvil owns the concrete LLM call. Temper owns workflow authority: this module
//! reads a [`WorkflowRoleDecisionRequest`], asks the provider for one manifest
//! action, and returns a [`WorkflowRoleDecisionReply`]. anvil does not receive or
//! execute Forge/workflow mutation tools; it only chooses `no_action` or one of
//! the request's authorized action names.

use std::time::Instant;

use serde::Deserialize;
use temper_protocol_decision::{
    WORKFLOW_ROLE_DECISION_NO_ACTION, WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION,
    WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
};

use crate::decision::{DecisionError, run_decision};
use crate::provider::{ProviderConfig, ProviderError};
use crate::workflow_role_decision_capture::{
    CaptureWriteResult, WorkflowRoleDecisionCapture, WorkflowRoleDecisionCaptureInput,
};
use crate::workflow_role_decision_observability::{
    ProviderCallLogOutcome, ReplyLogMetadata, WorkflowRoleTrace, capture_write_failed_event,
    capture_written_event, emit, provider_call_finish_event, provider_call_start_event,
    reply_event, request_event,
};
use crate::workflow_role_decision_prompt::{
    no_action_for_request, validated_reply_for_model_decision,
};

// Re-export the prompt/validation surface so callers (and lib.rs) see it on the
// `workflow_role_decision` module as before.
pub use crate::workflow_role_decision_prompt::{
    reply_for_model_decision, workflow_role_system_prompt, workflow_role_user_context,
};

// Internal prompt-module item the unit tests reach through `super::*`
// (`validated_reply_for_model_decision` is already imported above for `respond`,
// so the test's glob import sees it too).
#[cfg(test)]
use crate::workflow_role_decision_prompt::EXTERNAL_TOOL_SECTION;

/// Provider-backed workflow-role decision responder.
pub struct WorkflowRoleDecisionResponder {
    provider: ProviderConfig,
    capture: WorkflowRoleDecisionCapture,
}

impl WorkflowRoleDecisionResponder {
    /// Builds a responder using anvil's provider config, with redacted decision
    /// capture disabled.
    ///
    /// The capture dir is host config, not something this library reads from the
    /// environment: use [`with_capture_dir`](Self::with_capture_dir) to enable it.
    pub fn new(provider: ProviderConfig) -> Self {
        Self::with_capture_dir(provider, None::<std::path::PathBuf>)
    }

    /// Builds a responder, enabling redacted decision capture to `capture_dir`
    /// when supplied (the host-read `ANVIL_WORKFLOW_ROLE_DECISION_CAPTURE_DIR`).
    /// `None` (or an empty path) leaves capture disabled.
    pub fn with_capture_dir(
        provider: ProviderConfig,
        capture_dir: Option<impl Into<std::path::PathBuf>>,
    ) -> Self {
        Self {
            provider,
            capture: WorkflowRoleDecisionCapture::from_optional_dir(capture_dir),
        }
    }

    /// Runs one LLM-backed workflow-role decision.
    ///
    /// `handle` is the runtime spawn capability the nested decision run needs,
    /// passed explicitly from the caller's engine context.
    pub async fn respond(
        &self,
        handle: skein::runtime::RuntimeHandle,
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
            handle,
            &self.provider,
            &system_prompt,
            &user_context,
        )
        .await;
        let latency_ms = elapsed_ms(provider_call_started);
        let context = DecisionLogContext {
            request,
            trace: &trace,
            system_prompt: &system_prompt,
            user_context: &user_context,
            latency_ms,
        };

        match decision_result {
            Ok(decision) => Ok(self.finish_model_decision(&context, decision)),
            Err(DecisionError::Provider(error)) => Err(self.finish_provider_error(&context, error)),
            Err(error) => Ok(self.finish_decision_error(&context, &error)),
        }
    }

    /// Logs and captures a parseable model decision, returning the validated
    /// (authority-checked) reply.
    fn finish_model_decision(
        &self,
        context: &DecisionLogContext<'_>,
        decision: WorkflowRoleModelDecision,
    ) -> WorkflowRoleDecisionReply {
        let model_action = decision.action.trim().to_string();
        emit(provider_call_finish_event(
            context.request,
            context.trace,
            &self.provider,
            context.latency_ms,
            ProviderCallLogOutcome::Model {
                action: &model_action,
            },
        ));
        let model_decision = decision.clone();
        let validated = validated_reply_for_model_decision(context.request, decision);
        emit(reply_event(
            context.request,
            context.trace,
            &self.provider,
            &validated.reply,
            &validated.log_metadata,
        ));
        self.write_capture(context.capture_args(
            Some(&model_decision),
            Some(&validated.reply),
            validated.log_metadata.outcome,
            None,
        ));
        validated.reply
    }

    /// Logs and captures a provider-layer failure, surfacing it as a hard error
    /// (the worker should fail rather than silently no-op).
    fn finish_provider_error(
        &self,
        context: &DecisionLogContext<'_>,
        error: ProviderError,
    ) -> WorkflowRoleDecisionError {
        let error = DecisionError::Provider(error);
        emit(provider_call_finish_event(
            context.request,
            context.trace,
            &self.provider,
            context.latency_ms,
            ProviderCallLogOutcome::Error(&error),
        ));
        self.write_capture(context.capture_args(
            None,
            None,
            "provider_error",
            Some(decision_failure_class(&error)),
        ));
        WorkflowRoleDecisionError::Decision(error)
    }

    /// Logs and captures a non-provider decision failure (empty/unparseable
    /// reply), returning a safe `no_action` reply.
    fn finish_decision_error(
        &self,
        context: &DecisionLogContext<'_>,
        error: &DecisionError,
    ) -> WorkflowRoleDecisionReply {
        emit(provider_call_finish_event(
            context.request,
            context.trace,
            &self.provider,
            context.latency_ms,
            ProviderCallLogOutcome::Error(error),
        ));
        let reply = no_action_for_request(context.request, "decision failed");
        let log_metadata = ReplyLogMetadata::decision_error_no_action();
        emit(reply_event(
            context.request,
            context.trace,
            &self.provider,
            &reply,
            &log_metadata,
        ));
        self.write_capture(context.capture_args(
            None,
            Some(&reply),
            log_metadata.outcome,
            Some(decision_failure_class(error)),
        ));
        reply
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

/// Shared context for the post-provider-call logging/capture paths.
///
/// Bundles the request-scoped values every `finish_*` helper needs so they take
/// one borrow instead of five, and centralizes building [`DecisionCaptureArgs`]
/// for the always-present prompts and known latency.
struct DecisionLogContext<'a> {
    request: &'a WorkflowRoleDecisionRequest,
    trace: &'a WorkflowRoleTrace,
    system_prompt: &'a str,
    user_context: &'a str,
    latency_ms: u64,
}

impl<'a> DecisionLogContext<'a> {
    /// Builds capture args carrying this context's prompts and latency, with the
    /// outcome-specific decision/reply/outcome/failure-class supplied per call.
    fn capture_args(
        &self,
        model_decision: Option<&'a WorkflowRoleModelDecision>,
        final_reply: Option<&'a WorkflowRoleDecisionReply>,
        outcome: &'static str,
        failure_class: Option<&'static str>,
    ) -> DecisionCaptureArgs<'a> {
        DecisionCaptureArgs {
            request: self.request,
            trace: self.trace,
            system_prompt: Some(self.system_prompt),
            user_context: Some(self.user_context),
            model_decision,
            final_reply,
            latency_ms: Some(self.latency_ms),
            outcome,
            failure_class,
        }
    }
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
    /// The request uses a protocol version this anvil binary does not implement.
    UnsupportedProtocolVersion { actual: u32 },
    /// Building the provider or obtaining a model decision failed in a way that
    /// should fail the worker rather than silently no-op.
    Decision(DecisionError),
    /// anvil could not serialize the model context for the provider call.
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

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "workflow_role_decision_tests.rs"]
mod tests;
