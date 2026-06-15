//! Reply/error classification and structured logging for the decision adapter.

use std::time::Duration;

use super::error::WorkflowRoleDecisionProcessError;
use crate::{
    RoleDecisionReplyEvent, RoleDecisionRequestEvent, WorkflowRoleDecisionProtocolError,
    WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest, render_role_decision_reply_event,
    render_role_decision_request_event,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecisionDisposition {
    ExecuteAction,
    NoAction,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DecisionReplyClassification {
    pub(crate) validation_outcome: &'static str,
    pub(crate) action_kind: &'static str,
    pub(crate) disposition: DecisionDisposition,
    pub(crate) error: Option<String>,
}

pub(super) fn classify_decision_reply(
    request: &WorkflowRoleDecisionRequest,
    reply: &WorkflowRoleDecisionReply,
) -> DecisionReplyClassification {
    if reply.protocol_version != request.protocol_version {
        let error = WorkflowRoleDecisionProtocolError::VersionMismatch {
            expected: request.protocol_version,
            actual: reply.protocol_version,
        };
        return DecisionReplyClassification {
            validation_outcome: "protocol_mismatch",
            action_kind: "invalid_reply",
            disposition: DecisionDisposition::Error,
            error: Some(error.to_string()),
        };
    }
    if !request.action_is_authorized(&reply.action) {
        return DecisionReplyClassification {
            validation_outcome: "unauthorized_downgraded_to_no_action",
            action_kind: "no_action",
            disposition: DecisionDisposition::NoAction,
            error: None,
        };
    }
    if reply.is_no_action() {
        return DecisionReplyClassification {
            validation_outcome: "valid",
            action_kind: "no_action",
            disposition: DecisionDisposition::NoAction,
            error: None,
        };
    }
    DecisionReplyClassification {
        validation_outcome: "valid",
        action_kind: "authorized_action",
        disposition: DecisionDisposition::ExecuteAction,
        error: None,
    }
}

pub(super) fn classify_process_error(
    error: &WorkflowRoleDecisionProcessError,
) -> (&'static str, &'static str) {
    match error {
        WorkflowRoleDecisionProcessError::MalformedJson { .. } => {
            ("malformed_json", "invalid_reply")
        }
        WorkflowRoleDecisionProcessError::Timeout { .. } => ("timeout", "process_unavailable"),
        WorkflowRoleDecisionProcessError::Protocol(
            WorkflowRoleDecisionProtocolError::VersionMismatch { .. },
        ) => ("protocol_mismatch", "invalid_reply"),
        WorkflowRoleDecisionProcessError::Protocol(
            WorkflowRoleDecisionProtocolError::UnauthorizedAction { .. },
        ) => ("unauthorized_downgraded_to_no_action", "no_action"),
        WorkflowRoleDecisionProcessError::InvalidConfig { .. }
        | WorkflowRoleDecisionProcessError::Io { .. }
        | WorkflowRoleDecisionProcessError::Exit { .. } => {
            ("process_failure", "process_unavailable")
        }
    }
}

pub(super) fn log_decision_request(
    identity: &crate::WorkItemIdentity,
    request: &WorkflowRoleDecisionRequest,
) {
    let authorized_actions = request
        .authorized_actions
        .iter()
        .map(|action| action.action.clone())
        .collect::<Vec<_>>();
    let available_external_tools = request
        .available_external_tools
        .iter()
        .map(|tool| tool.id.to_string())
        .collect::<Vec<_>>();
    eprintln!(
        "{}",
        render_role_decision_request_event(&RoleDecisionRequestEvent {
            identity,
            workflow_id: &request.workflow_id,
            authorized_actions: &authorized_actions,
            available_external_tools: &available_external_tools,
        })
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn log_decision_reply(
    identity: &crate::WorkItemIdentity,
    selected_action: Option<&str>,
    validation_outcome: &str,
    action_kind: &str,
    reason: Option<&str>,
    latency: Duration,
    error: Option<&str>,
) {
    eprintln!(
        "{}",
        render_role_decision_reply_event(&RoleDecisionReplyEvent {
            identity,
            selected_action,
            validation_outcome,
            action_kind,
            reason,
            latency,
            error,
        })
    );
}
