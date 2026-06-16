//! Reply/error classification and structured logging for the decision adapter.
//!
//! The two `log_*` helpers map the role-decision process round-trip onto the
//! §7 `agent` events: a request becomes `agent.started`, the reply (or the
//! process error) becomes `agent.finished`. The runner-side decision process is
//! the daemon's `agent` plane, so its run `kind` is `decision`; the role comes
//! from the work-item identity and the duration from the measured latency.

use std::time::Duration;

use temper_log::emit::{AgentFinished, AgentStarted, emit_agent_finished, emit_agent_started};

use super::error::WorkflowRoleDecisionProcessError;
use crate::observability::work_item_ref;
use crate::{
    WorkflowRoleDecisionProtocolError, WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
};

/// Run `kind` for the role-decision process agent (the daemon's `agent` plane).
pub(crate) const DECISION_RUN_KIND: &str = "decision";

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

/// Emits `agent.started` for a role-decision request (§7 `agent` plane).
pub(super) fn log_decision_request(
    identity: &crate::WorkItemIdentity,
    request: &WorkflowRoleDecisionRequest,
) {
    let item = work_item_ref(identity);
    let detail = format!(
        "deciding {} ({} actions authorized)",
        request.workflow_id,
        request.authorized_actions.len()
    );
    emit_agent_started(AgentStarted {
        item: &item,
        role: identity.role.as_str(),
        kind: DECISION_RUN_KIND,
        detail: &detail,
    });
}

/// Emits `agent.finished` for a role-decision reply or process error.
///
/// The numeric `duration_ms` carries the measured round-trip latency; the
/// summary is the verdict/outcome, the reply reason, or the error preview.
#[allow(clippy::too_many_arguments)]
pub(super) fn log_decision_reply(
    identity: &crate::WorkItemIdentity,
    selected_action: Option<&str>,
    validation_outcome: &str,
    _action_kind: &str,
    reason: Option<&str>,
    latency: Duration,
    error: Option<&str>,
) {
    let item = work_item_ref(identity);
    let summary = decision_summary(selected_action, validation_outcome, reason, error);
    emit_agent_finished(AgentFinished {
        item: &item,
        role: identity.role.as_str(),
        kind: DECISION_RUN_KIND,
        duration_ms: duration_millis(latency),
        summary: &summary,
    });
}

/// Builds the one-line `agent.finished` summary from the decision outcome.
///
/// Errors win (they are the operator's concern); otherwise the chosen action
/// and reply reason are named, falling back to the validation outcome. The
/// human renderer redacts this free text.
fn decision_summary(
    selected_action: Option<&str>,
    validation_outcome: &str,
    reason: Option<&str>,
    error: Option<&str>,
) -> String {
    if let Some(error) = error {
        return format!("error={error}");
    }
    let action = selected_action.unwrap_or("no_action");
    match reason {
        Some(reason) if !reason.is_empty() => format!("action={action} | {reason}"),
        _ => format!("action={action} verdict={validation_outcome}"),
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
