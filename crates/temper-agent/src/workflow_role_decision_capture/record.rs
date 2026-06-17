//! The serializable capture-file schema.
//!
//! [`DecisionCaptureFile`] is the on-disk shape (schema v1): bounded, redacted
//! previews only — never raw prompts, bodies, or credentials. All text passes
//! through [`redacted_preview`], and scalar fields through [`bounded`].

use serde::Serialize;
use temper_protocol_decision::{
    WORKFLOW_ROLE_DECISION_NO_ACTION, WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
};

use crate::observability::{FIELD_PREVIEW_CHARS, redacted_preview};
use crate::workflow_role_decision::WorkflowRoleModelDecision;
use crate::workflow_role_decision_observability::WorkflowRoleTrace;

use super::WorkflowRoleDecisionCaptureInput;

const CAPTURE_SCHEMA_VERSION: u32 = 1;
const CAPTURE_PREVIEW_CHARS: usize = 1_000;

#[derive(Serialize)]
pub(crate) struct DecisionCaptureFile {
    schema_version: u32,
    captured_at_unix_ms: u64,
    trace: CaptureTrace,
    workflow: CaptureWorkflow,
    provider: CaptureProvider,
    allowed_actions: Vec<String>,
    available_external_tool_ids: Vec<String>,
    pub(crate) prompt: Option<TextCapture>,
    context: Option<TextCapture>,
    model_decision: Option<ModelDecisionCapture>,
    final_reply: Option<ReplyCapture>,
    latency_ms: Option<u64>,
    outcome: &'static str,
    failure_class: Option<&'static str>,
}

impl DecisionCaptureFile {
    pub(crate) fn from_input(
        input: WorkflowRoleDecisionCaptureInput<'_>,
        captured_at_unix_ms: u64,
    ) -> Self {
        let identity = input.provider.observability_identity();
        Self {
            schema_version: CAPTURE_SCHEMA_VERSION,
            captured_at_unix_ms,
            trace: CaptureTrace::from_trace(input.trace),
            workflow: CaptureWorkflow::from_request(input.request, input.trace),
            provider: CaptureProvider {
                provider_id: bounded(identity.provider_id),
                model_id: bounded(identity.model_id),
                auth_mode: identity.auth_mode,
            },
            allowed_actions: allowed_action_names(input.request),
            available_external_tool_ids: external_tool_ids(input.request),
            prompt: input.system_prompt.map(TextCapture::from_text),
            context: input.user_context.map(TextCapture::from_text),
            model_decision: input
                .model_decision
                .map(ModelDecisionCapture::from_decision),
            final_reply: input.final_reply.map(ReplyCapture::from_reply),
            latency_ms: input.latency_ms,
            outcome: input.outcome,
            failure_class: input.failure_class,
        }
    }
}

#[derive(Serialize)]
struct CaptureTrace {
    run_id: Option<String>,
    tick_id: Option<String>,
    work_item_id: Option<String>,
    decision_id: Option<String>,
}

impl CaptureTrace {
    fn from_trace(trace: &WorkflowRoleTrace) -> Self {
        Self {
            run_id: trace.run_id.as_deref().map(bounded),
            tick_id: trace.tick_id.as_deref().map(bounded),
            work_item_id: trace.work_item_id.as_deref().map(bounded),
            decision_id: trace.decision_id.as_deref().map(bounded),
        }
    }
}

#[derive(Serialize)]
struct CaptureWorkflow {
    workflow_id: String,
    role_id: String,
    work_item_role: Option<String>,
    repository: Option<String>,
    queue: Option<String>,
    kind: Option<String>,
    artifact: CaptureArtifact,
}

impl CaptureWorkflow {
    fn from_request(request: &WorkflowRoleDecisionRequest, trace: &WorkflowRoleTrace) -> Self {
        Self {
            workflow_id: bounded(&request.workflow_id),
            role_id: bounded(request.role_manifest.id.as_str()),
            work_item_role: trace.role.as_deref().map(bounded),
            repository: trace.repository.as_deref().map(bounded),
            queue: trace.queue.as_deref().map(bounded),
            kind: trace.kind.as_deref().map(bounded),
            artifact: CaptureArtifact {
                artifact_type: trace.artifact_type.as_deref().map(bounded),
                number: trace.artifact_number.as_deref().map(bounded),
            },
        }
    }
}

#[derive(Serialize)]
struct CaptureArtifact {
    artifact_type: Option<String>,
    number: Option<String>,
}

#[derive(Serialize)]
struct CaptureProvider {
    provider_id: String,
    model_id: String,
    auth_mode: &'static str,
}

#[derive(Serialize)]
pub(crate) struct TextCapture {
    chars: usize,
    pub(crate) preview: String,
}

impl TextCapture {
    fn from_text(text: &str) -> Self {
        Self {
            chars: text.chars().count(),
            preview: redacted_preview(text, CAPTURE_PREVIEW_CHARS),
        }
    }
}

#[derive(Serialize)]
struct ModelDecisionCapture {
    action: String,
    reason_preview: String,
}

impl ModelDecisionCapture {
    fn from_decision(decision: &WorkflowRoleModelDecision) -> Self {
        Self {
            action: bounded(&decision.action),
            reason_preview: redacted_preview(&decision.reason, FIELD_PREVIEW_CHARS),
        }
    }
}

#[derive(Serialize)]
struct ReplyCapture {
    action: String,
    reason_preview: String,
}

impl ReplyCapture {
    fn from_reply(reply: &WorkflowRoleDecisionReply) -> Self {
        Self {
            action: bounded(&reply.action),
            reason_preview: redacted_preview(&reply.reason, FIELD_PREVIEW_CHARS),
        }
    }
}

fn bounded(value: &str) -> String {
    redacted_preview(value, FIELD_PREVIEW_CHARS)
}

fn allowed_action_names(request: &WorkflowRoleDecisionRequest) -> Vec<String> {
    std::iter::once(WORKFLOW_ROLE_DECISION_NO_ACTION.to_string())
        .chain(
            request
                .authorized_actions
                .iter()
                .map(|action| bounded(&action.action)),
        )
        .collect()
}

fn external_tool_ids(request: &WorkflowRoleDecisionRequest) -> Vec<String> {
    request
        .available_external_tools
        .iter()
        .map(|tool| bounded(tool.id.as_str()))
        .collect()
}
