//! Structured observability logging for process-backed role-decision actions.
//!
//! The applied-transition path is a §7 `engine` state change and emits
//! `transition.applied` with real fields (transition id, label delta from the
//! applied effects). Dispatch routing, no-ops, stale work, and failures are
//! *causes between* state changes, so they stay at `debug` under the engine
//! target with structured fields (no §7 info catalog entry).

use temper_workflow::{ExecutionError, ExecutionReport, ToolManifest, TransitionId, VerdictId};

use temper_log::emit::{TransitionApplied, emit_transition_applied};

use crate::observability::{labels_delta, work_item_ref};
use crate::{
    WorkItemIdentity, execution_error_diagnostic_classes, execution_error_failure_class,
    postcondition_outcome_for_error,
};

pub(super) fn log_verdict_route(
    identity: &WorkItemIdentity,
    action_transition: &TransitionId,
    routed: &TransitionId,
    verdict: Option<&VerdictId>,
) {
    let verdict = verdict.map(VerdictId::as_str).unwrap_or("");
    tracing::debug!(
        target: "temper::engine",
        repo = identity.repo.as_str(),
        artifact.ref = %work_item_ref(identity),
        role = identity.role.as_str(),
        selected_action = action_transition.as_str(),
        transition = routed.as_str(),
        verdict = verdict,
        outcome = "verdict_routed",
        "dispatch: verdict routed {} -> {routed}",
        action_transition.as_str(),
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
    tracing::debug!(
        target: "temper::engine",
        repo = identity.repo.as_str(),
        artifact.ref = %work_item_ref(identity),
        role = identity.role.as_str(),
        selected_action = tool.name.as_str(),
        transition = %tool.transition,
        external_executor_required,
        external_executor = external_executor_id,
        external_executor_available,
        no_op_reason,
        outcome,
        "dispatch: {} -> {} ({outcome})",
        tool.name,
        tool.transition,
    );
}

pub(super) fn log_transition_success(identity: &WorkItemIdentity, report: &ExecutionReport) {
    let item = work_item_ref(identity);
    let delta = labels_delta(&report.applied);
    emit_transition_applied(TransitionApplied {
        item: &item,
        transition: report.transition.as_str(),
        detail: "",
        labels_delta: &delta,
    });
}

pub(super) fn log_transition_error(
    identity: &WorkItemIdentity,
    transition: &TransitionId,
    error: &ExecutionError,
    stale_work: bool,
) {
    let failure_class = execution_error_failure_class(error);
    let diagnostics = execution_error_diagnostic_classes(error).join(",");
    tracing::debug!(
        target: "temper::engine",
        repo = identity.repo.as_str(),
        artifact.ref = %work_item_ref(identity),
        role = identity.role.as_str(),
        transition = transition.as_str(),
        outcome = if stale_work { "stale_no_op" } else { "failed" },
        stale_work,
        failure_class = failure_class.as_str(),
        diagnostic_classes = diagnostics.as_str(),
        postcondition_outcome = postcondition_outcome_for_error(error),
        "transition {transition} {}",
        if stale_work { "ignored as stale" } else { "failed" },
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
    tracing::debug!(
        target: "temper::engine",
        repo = identity.repo.as_str(),
        artifact.ref = %work_item_ref(identity),
        role = identity.role.as_str(),
        transition = transition.as_str(),
        outcome,
        stale_work,
        failure_class,
        postcondition_outcome,
        "transition {transition} {outcome}",
    );
}

#[cfg(test)]
mod tests {
    /// The effect-summary helper still feeds debug-level diagnostics; assert it
    /// stays body-free so the migration kept the redaction property.
    #[test]
    fn effect_summary_is_body_free() {
        use crate::observability::workflow_effect_summary;
        use temper_workflow::{LabelId, WorkflowEffect};
        let summary = workflow_effect_summary(&[WorkflowEffect::AddLabel(LabelId::new("ready"))]);
        assert_eq!(summary, vec!["add_label:ready"]);
    }
}
