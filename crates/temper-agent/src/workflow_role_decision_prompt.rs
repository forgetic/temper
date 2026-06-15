//! Workflow-role decision prompt/context construction and reply validation.
//!
//! Builds the runtime system prompt (folding the bound external-tool section
//! into the role manifest), the JSON user context, and validates a model
//! decision into a [`WorkflowRoleDecisionReply`] — downgrading any unauthorized
//! action to `no_action`. The orchestration that calls these (logging, capture,
//! the provider call) lives in [`crate::workflow_role_decision`].

use temper_process_protocol::{
    BoundExternalTool, WORKFLOW_ROLE_DECISION_NO_ACTION, WorkflowRoleDecisionReply,
    WorkflowRoleDecisionRequest, WorkflowRoleManifest,
};

use crate::observability::{REASON_PREVIEW_CHARS, redacted_preview};
use crate::workflow_role_decision::WorkflowRoleModelDecision;
use crate::workflow_role_decision_observability::ReplyLogMetadata;

pub(crate) const EXTERNAL_TOOL_SECTION: &str = "User-declared external tools";
const CODING_WORKSPACE_TOOL_ID: &str = "coding_workspace";

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
pub(crate) struct ValidatedWorkflowRoleReply {
    pub(crate) reply: WorkflowRoleDecisionReply,
    pub(crate) log_metadata: ReplyLogMetadata,
}

pub(crate) fn validated_reply_for_model_decision(
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

pub(crate) fn no_action_for_request(
    request: &WorkflowRoleDecisionRequest,
    reason: impl Into<String>,
) -> WorkflowRoleDecisionReply {
    WorkflowRoleDecisionReply {
        protocol_version: request.protocol_version,
        action: WORKFLOW_ROLE_DECISION_NO_ACTION.to_string(),
        reason: reason.into(),
    }
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
