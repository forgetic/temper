//! Stable JSON renderers for runner observability events.

use temper_workflow::{
    ArtifactSource, ReconcileFinding, RecoveryAction, TransitionId, WorkflowEffect,
};

use super::{
    ActionDispatchEvent, MechanicalReconciliationEvent, RoleDecisionReplyEvent,
    RoleDecisionRequestEvent, ScanSummaryEvent, TransitionExecutionEvent, WorkItemSelectedEvent,
};
use crate::observability::{StructuredEvent, WorkItemIdentity, redacted_preview};
use std::time::Duration;

const PREVIEW_LIMIT: usize = 240;

/// Renders a bounded structured event before invoking a decision process.
pub fn render_role_decision_request_event(event: &RoleDecisionRequestEvent<'_>) -> String {
    work_item_event("role_decision_request", event.identity)
        .string("workflow_id", event.workflow_id)
        .number(
            "authorized_action_count",
            saturating_u64(event.authorized_actions.len()),
        )
        .string_array(
            "authorized_actions",
            event.authorized_actions.iter().cloned(),
        )
        .number(
            "available_external_tool_count",
            saturating_u64(event.available_external_tools.len()),
        )
        .string_array(
            "available_external_tools",
            event.available_external_tools.iter().cloned(),
        )
        .render()
}

/// Renders a bounded structured event after a role scan completes.
pub fn render_scan_summary_event(event: &ScanSummaryEvent<'_>) -> String {
    StructuredEvent::new("scan_summary")
        .optional_string("tick_id", event.tick_id.map(ToOwned::to_owned))
        .string("worker_kind", event.worker_kind)
        .string("worker", event.worker)
        .string("repo", event.repo.to_string())
        .string("workflow_id", event.workflow_id)
        .optional_string("role", event.role.map(ToOwned::to_owned))
        .number("work_item_count", saturating_u64(event.work_item_count))
        .render()
}

/// Renders a bounded structured event for one selected scan work item.
pub fn render_work_item_selected_event(event: &WorkItemSelectedEvent<'_>) -> String {
    work_item_event("work_item_selected", event.identity)
        .string("workflow_id", event.workflow_id)
        .string("worker", event.worker)
        .render()
}

/// Renders a bounded structured event after a decision process returns or fails.
pub fn render_role_decision_reply_event(event: &RoleDecisionReplyEvent<'_>) -> String {
    let mut rendered = work_item_event("role_decision_reply", event.identity)
        .optional_string(
            "selected_action",
            event.selected_action.map(ToOwned::to_owned),
        )
        .string("validation_outcome", event.validation_outcome)
        .string("action_kind", event.action_kind)
        .number("latency_ms", duration_millis(event.latency));
    if let Some(reason) = event.reason {
        rendered = rendered.string("reason_preview", redacted_preview(reason, PREVIEW_LIMIT));
    }
    if let Some(error) = event.error {
        rendered = rendered.string("error_preview", redacted_preview(error, PREVIEW_LIMIT));
    }
    rendered.render()
}

/// Renders a structured action-to-transition dispatch event.
pub fn render_action_dispatch_event(event: &ActionDispatchEvent<'_>) -> String {
    let mut rendered = work_item_event("action_dispatch", event.identity)
        .string("selected_action", event.selected_action)
        .string("transition", event.transition.to_string())
        .boolean(
            "external_executor_required",
            event.external_executor_required,
        )
        .string("outcome", event.outcome);
    if let Some(executor) = event.external_executor_id {
        rendered = rendered.string("external_executor", executor);
    }
    if let Some(available) = event.external_executor_available {
        rendered = rendered.boolean("external_executor_available", available);
    }
    if let Some(reason) = event.no_op_reason {
        rendered = rendered.string("no_op_reason", reason);
    }
    rendered.render()
}

/// Renders a structured transition execution outcome event.
pub fn render_transition_execution_event(event: &TransitionExecutionEvent<'_>) -> String {
    let mut rendered = work_item_event("transition_execution", event.identity)
        .string("transition", event.transition.to_string())
        .string("outcome", event.outcome)
        .boolean("stale_work", event.stale_work)
        .number("effect_count", saturating_u64(event.effects.len()))
        .string_array("effects", workflow_effect_summary(event.effects))
        .string("postcondition_outcome", event.postcondition_outcome);
    if let Some(failure_class) = event.failure_class {
        rendered = rendered.string("failure_class", failure_class);
    }
    if !event.diagnostic_classes.is_empty() {
        rendered = rendered
            .number(
                "diagnostic_count",
                saturating_u64(event.diagnostic_classes.len()),
            )
            .string_array("diagnostic_classes", event.diagnostic_classes.clone());
    }
    rendered.render()
}

/// Renders a structured mechanical reconciliation finding/action event.
pub fn render_mechanical_reconciliation_event(event: &MechanicalReconciliationEvent<'_>) -> String {
    let mut rendered = StructuredEvent::new("mechanical_reconciliation")
        .string("worker_kind", "mechanical")
        .string("worker", event.worker)
        .string("repo", event.repo.to_string())
        .string("finding", finding_name(event.finding))
        .string("action", action_name(event.action));

    if let Some(target) = finding_target(event.finding).or_else(|| action_target(event.action)) {
        let (artifact_type, artifact_number) = source_parts(target);
        rendered = rendered
            .string("artifact_type", artifact_type)
            .number("artifact_number", artifact_number.get());
    }
    if let Some(transition) = finding_transition(event.finding) {
        rendered = rendered.string("transition", transition.to_string());
    }
    if let ReconcileFinding::BlockedWithoutDependencies {
        dependency_count,
        relation_count,
        ..
    } = event.finding
    {
        rendered = rendered
            .string("diagnostic", "blocked_artifact_without_dependencies")
            .number("dependency_count", saturating_u64(*dependency_count))
            .number("relation_count", saturating_u64(*relation_count))
            .string(
                "reason",
                "dependency-gated unblocking intentionally cannot proceed without at least one recorded dependency",
            );
    }
    if let Some(effect_count) = action_effect_count(event.action) {
        rendered = rendered.number("effect_count", saturating_u64(effect_count));
    }
    rendered.render()
}

/// Returns compact, body-free descriptions of planned/applied effects.
pub fn workflow_effect_summary(effects: &[WorkflowEffect]) -> Vec<String> {
    effects.iter().map(summarize_effect).collect()
}

fn finding_name(finding: &ReconcileFinding) -> &'static str {
    match finding {
        ReconcileFinding::ExpiredLease { .. } => "expired_lease",
        ReconcileFinding::ImpossibleState { .. } => "impossible_state",
        ReconcileFinding::ClassificationDrift { .. } => "classification_drift",
        ReconcileFinding::BlockedWithoutDependencies { .. } => "blocked_without_dependencies",
        ReconcileFinding::PartialTransition { .. } => "partial_transition",
        ReconcileFinding::StaleCommand { .. } => "stale_command",
        ReconcileFinding::DependenciesResolved { .. } => "dependencies_resolved",
    }
}

fn action_name(action: &RecoveryAction) -> &'static str {
    match action {
        RecoveryAction::RequeueLease { .. } => "requeue_lease",
        RecoveryAction::Escalate { .. } => "escalate",
        RecoveryAction::Repair { .. } => "repair",
        RecoveryAction::MarkReconciled { .. } => "mark_reconciled",
        RecoveryAction::Unblock { .. } => "unblock",
        RecoveryAction::Diagnose { .. } => "diagnose",
    }
}

fn finding_target(finding: &ReconcileFinding) -> Option<ArtifactSource> {
    match finding {
        ReconcileFinding::ExpiredLease { target, .. }
        | ReconcileFinding::ImpossibleState { target, .. }
        | ReconcileFinding::ClassificationDrift { target, .. }
        | ReconcileFinding::BlockedWithoutDependencies { target, .. }
        | ReconcileFinding::PartialTransition { target, .. }
        | ReconcileFinding::StaleCommand { target, .. }
        | ReconcileFinding::DependenciesResolved { target, .. } => Some(*target),
    }
}

fn action_target(action: &RecoveryAction) -> Option<ArtifactSource> {
    match action {
        RecoveryAction::RequeueLease { target }
        | RecoveryAction::Escalate { target, .. }
        | RecoveryAction::Repair { target, .. }
        | RecoveryAction::Unblock { target, .. }
        | RecoveryAction::Diagnose { target, .. } => Some(*target),
        RecoveryAction::MarkReconciled { .. } => None,
    }
}

fn finding_transition(finding: &ReconcileFinding) -> Option<&TransitionId> {
    match finding {
        ReconcileFinding::BlockedWithoutDependencies { transition, .. }
        | ReconcileFinding::DependenciesResolved { transition, .. } => Some(transition),
        _ => None,
    }
}

fn action_effect_count(action: &RecoveryAction) -> Option<usize> {
    match action {
        RecoveryAction::Repair { effects, .. } | RecoveryAction::Unblock { effects, .. } => {
            Some(effects.len())
        }
        _ => None,
    }
}

fn source_parts(source: ArtifactSource) -> (&'static str, temper_forge::ItemNumber) {
    match source {
        ArtifactSource::Issue { number } => ("issue", number),
        ArtifactSource::PullRequest { number } => ("pull_request", number),
    }
}

fn work_item_event(event: &str, identity: &WorkItemIdentity) -> StructuredEvent {
    StructuredEvent::new(event)
        .optional_string("tick_id", identity.tick_id.clone())
        .string("work_item_id", identity.work_item_id.clone())
        .string("decision_id", identity.decision_id.clone())
        .string("repo", identity.repo.to_string())
        .string("role", identity.role.to_string())
        .string("queue", identity.queue.to_string())
        .string("artifact_type", identity.artifact_type.as_str())
        .number("artifact_number", identity.artifact_number.get())
        .string("artifact_kind", identity.artifact_kind.to_string())
}

fn summarize_effect(effect: &WorkflowEffect) -> String {
    match effect {
        WorkflowEffect::AddLabel(label) => format!("add_label:{label}"),
        WorkflowEffect::RemoveLabel(label) => format!("remove_label:{label}"),
        WorkflowEffect::SetAssignee { role } => format!("set_assignee:{role}"),
        WorkflowEffect::RemoveAssignee { role } => format!("remove_assignee:{role}"),
        WorkflowEffect::CreateComment { .. } => "create_comment".to_string(),
        WorkflowEffect::CreateIssue { .. } => "create_issue".to_string(),
        WorkflowEffect::CreatePullRequest { .. } => "create_pull_request".to_string(),
        WorkflowEffect::RequestReviewers { roles } => format!(
            "request_reviewers:{}",
            roles
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        WorkflowEffect::SubmitReview { decision } => format!("submit_review:{decision:?}"),
        WorkflowEffect::SetBody { .. } => "set_body".to_string(),
        WorkflowEffect::AttachReview { decision, .. } => format!("attach_review:{decision:?}"),
        WorkflowEffect::CreateIssues { .. } => "create_issues".to_string(),
        WorkflowEffect::UpdateLease { .. } => "update_lease".to_string(),
        WorkflowEffect::ReleaseLease => "release_lease".to_string(),
        WorkflowEffect::MergePullRequest => "merge_pull_request".to_string(),
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
