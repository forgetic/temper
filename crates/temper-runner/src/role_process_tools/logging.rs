//! Structured observability logging for process-backed role-decision actions.

use temper_workflow::{ExecutionError, ExecutionReport, ToolManifest, TransitionId, VerdictId};

use crate::{
    ActionDispatchEvent, TransitionExecutionEvent, WorkItemIdentity,
    execution_error_diagnostic_classes, execution_error_failure_class,
    postcondition_outcome_for_error, render_action_dispatch_event,
    render_transition_execution_event,
};

pub(super) fn log_verdict_route(
    identity: &WorkItemIdentity,
    action_transition: &TransitionId,
    routed: &TransitionId,
    verdict: Option<&VerdictId>,
) {
    let verdict = verdict.map(VerdictId::as_str).unwrap_or("");
    tracing::info!(
        target: "temper_runner",
        "{}",
        render_action_dispatch_event(&ActionDispatchEvent {
            identity,
            selected_action: action_transition.as_str(),
            transition: routed,
            external_executor_required: true,
            external_executor_id: None,
            external_executor_available: Some(true),
            outcome: "verdict_routed",
            no_op_reason: (!verdict.is_empty()).then_some(verdict),
        })
    );
}

pub(super) fn log_action_dispatch(
    identity: &WorkItemIdentity,
    tool: &ToolManifest,
    external_executor_required: bool,
    external_executor_id: Option<&str>,
    external_executor_available: Option<bool>,
    outcome: &str,
    no_op_reason: Option<&str>,
) {
    tracing::info!(
        target: "temper_runner",
        "{}",
        render_action_dispatch_event(&ActionDispatchEvent {
            identity,
            selected_action: &tool.name,
            transition: &tool.transition,
            external_executor_required,
            external_executor_id,
            external_executor_available,
            outcome,
            no_op_reason,
        })
    );
}

pub(super) fn log_transition_success(identity: &WorkItemIdentity, report: &ExecutionReport) {
    tracing::info!(
        target: "temper_runner",
        "{}",
        render_transition_execution_event(&TransitionExecutionEvent {
            identity,
            transition: &report.transition,
            outcome: "mutated",
            stale_work: false,
            effects: &report.applied,
            failure_class: None,
            diagnostic_classes: Vec::new(),
            postcondition_outcome: "passed",
        })
    );
}

pub(super) fn log_transition_error(
    identity: &WorkItemIdentity,
    transition: &TransitionId,
    error: &ExecutionError,
    stale_work: bool,
) {
    let failure_class = execution_error_failure_class(error);
    tracing::info!(
        target: "temper_runner",
        "{}",
        render_transition_execution_event(&TransitionExecutionEvent {
            identity,
            transition,
            outcome: if stale_work { "stale_no_op" } else { "failed" },
            stale_work,
            effects: &[],
            failure_class: Some(failure_class.as_str()),
            diagnostic_classes: execution_error_diagnostic_classes(error),
            postcondition_outcome: postcondition_outcome_for_error(error),
        })
    );
}

pub(super) fn log_transition_custom(
    identity: &WorkItemIdentity,
    transition: &TransitionId,
    outcome: &str,
    stale_work: bool,
    failure_class: Option<&str>,
    postcondition_outcome: &str,
) {
    tracing::info!(
        target: "temper_runner",
        "{}",
        render_transition_execution_event(&TransitionExecutionEvent {
            identity,
            transition,
            outcome,
            stale_work,
            effects: &[],
            failure_class,
            diagnostic_classes: Vec::new(),
            postcondition_outcome,
        })
    );
}
