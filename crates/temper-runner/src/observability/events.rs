use std::time::Duration;

use temper_forge::RepositoryId;
use temper_workflow::{
    ArtifactSource, ExecutionError, PlanDiagnostic, ReconcileFinding, RecoveryAction, TransitionId,
    WorkflowEffect,
};

use super::{StructuredEvent, WorkItemIdentity, redacted_preview};

const PREVIEW_LIMIT: usize = 240;

/// Render input for a role-decision request event.
pub struct RoleDecisionRequestEvent<'a> {
    pub identity: &'a WorkItemIdentity,
    pub workflow_id: &'a str,
    pub authorized_actions: &'a [String],
    pub available_external_tools: &'a [String],
}

/// Render input for a role scan summary event.
pub struct ScanSummaryEvent<'a> {
    pub tick_id: Option<&'a str>,
    pub worker_kind: &'a str,
    pub worker: &'a str,
    pub repo: &'a RepositoryId,
    pub workflow_id: &'a str,
    pub role: Option<&'a str>,
    pub work_item_count: usize,
}

/// Render input for a role scan work-item selection event.
pub struct WorkItemSelectedEvent<'a> {
    pub identity: &'a WorkItemIdentity,
    pub workflow_id: &'a str,
    pub worker: &'a str,
}

/// Render input for a role-decision reply event.
pub struct RoleDecisionReplyEvent<'a> {
    pub identity: &'a WorkItemIdentity,
    pub selected_action: Option<&'a str>,
    pub validation_outcome: &'a str,
    pub action_kind: &'a str,
    pub reason: Option<&'a str>,
    pub latency: Duration,
    pub error: Option<&'a str>,
}

/// Render input for mapping a selected action to a manifest transition.
pub struct ActionDispatchEvent<'a> {
    pub identity: &'a WorkItemIdentity,
    pub selected_action: &'a str,
    pub transition: &'a TransitionId,
    pub external_executor_required: bool,
    pub external_executor_id: Option<&'a str>,
    pub external_executor_available: Option<bool>,
    pub outcome: &'a str,
    pub no_op_reason: Option<&'a str>,
}

/// Render input for the outcome of a transition execution attempt.
pub struct TransitionExecutionEvent<'a> {
    pub identity: &'a WorkItemIdentity,
    pub transition: &'a TransitionId,
    pub outcome: &'a str,
    pub stale_work: bool,
    pub effects: &'a [WorkflowEffect],
    pub failure_class: Option<&'a str>,
    pub diagnostic_classes: Vec<String>,
    pub postcondition_outcome: &'a str,
}

/// Render input for a mechanical reconciler finding/action pair.
pub struct MechanicalReconciliationEvent<'a> {
    pub worker: &'a str,
    pub repo: &'a RepositoryId,
    pub finding: &'a ReconcileFinding,
    pub action: &'a RecoveryAction,
}

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

/// Returns a stable high-level failure class for an execution error.
pub fn execution_error_failure_class(error: &ExecutionError) -> String {
    match error {
        ExecutionError::Validation { diagnostics } => diagnostics
            .first()
            .map(|diagnostic| format!("validation:{}", plan_diagnostic_class(diagnostic)))
            .unwrap_or_else(|| "validation".to_string()),
        ExecutionError::Precondition { diagnostics } => diagnostics
            .first()
            .map(|diagnostic| format!("precondition:{}", plan_diagnostic_class(diagnostic)))
            .unwrap_or_else(|| "precondition".to_string()),
        ExecutionError::Classification(_) => "classification".to_string(),
        ExecutionError::TargetMissing { .. } => "target_missing".to_string(),
        ExecutionError::TargetStale { .. } => "target_stale".to_string(),
        ExecutionError::MergeConflict { .. } => "merge_conflict".to_string(),
        ExecutionError::UnsupportedEffect { .. } => "unsupported_effect".to_string(),
        ExecutionError::UnresolvedAssignee { .. } => "unresolved_assignee".to_string(),
        ExecutionError::UnresolvedReviewer { .. } => "unresolved_reviewer".to_string(),
        ExecutionError::MissingCorrelationKey { .. } => "missing_correlation_key".to_string(),
        ExecutionError::UnresolvedPullRequestCreate { .. } => {
            "unresolved_pull_request_create".to_string()
        }
        ExecutionError::UnresolvedSetBody { .. } => "unresolved_set_body".to_string(),
        ExecutionError::UnresolvedAttachReview { .. } => "unresolved_attach_review".to_string(),
        ExecutionError::UnresolvedCreateIssues { .. } => "unresolved_create_issues".to_string(),
        ExecutionError::UnknownCreateIssuesDependency { .. } => {
            "unknown_create_issues_dependency".to_string()
        }
        ExecutionError::PostconditionFailed { .. } => "postcondition_failed".to_string(),
        ExecutionError::Backend { .. } => "backend".to_string(),
    }
}

/// Returns compact diagnostic classes without full diagnostic prose.
pub fn execution_error_diagnostic_classes(error: &ExecutionError) -> Vec<String> {
    match error {
        ExecutionError::Validation { diagnostics }
        | ExecutionError::Precondition { diagnostics } => diagnostics
            .iter()
            .map(|diagnostic| plan_diagnostic_class(diagnostic).to_string())
            .collect(),
        _ => Vec::new(),
    }
}

/// Returns the postcondition outcome implied by an execution error.
pub fn postcondition_outcome_for_error(error: &ExecutionError) -> &'static str {
    if matches!(error, ExecutionError::PostconditionFailed { .. }) {
        "failed"
    } else {
        "not_checked"
    }
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

fn plan_diagnostic_class(diagnostic: &PlanDiagnostic) -> &'static str {
    match diagnostic {
        PlanDiagnostic::UnknownTransition { .. } => "unknown_transition",
        PlanDiagnostic::Unauthorized { .. } => "unauthorized",
        PlanDiagnostic::ArtifactKindMismatch { .. } => "artifact_kind_mismatch",
        PlanDiagnostic::StalePrecondition { .. } => "stale_precondition",
        PlanDiagnostic::ContradictedPrecondition { .. } => "contradicted_precondition",
        PlanDiagnostic::GateNotSatisfied { .. } => "gate_not_satisfied",
        PlanDiagnostic::ImpossibleState { .. } => "impossible_state",
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_forge::ItemNumber;
    use temper_workflow::{
        ArtifactKindId, ArtifactSource, LabelId, Postcondition, QueueId, RoleId,
    };

    fn identity() -> WorkItemIdentity {
        WorkItemIdentity::new(
            &temper_forge::RepositoryId::new("forgejo:acme/service"),
            &RoleId::new("engineer"),
            &QueueId::new("ready"),
            ArtifactSource::Issue {
                number: ItemNumber::new(42),
            },
            &ArtifactKindId::new("code"),
        )
        .with_tick_id("tick-1")
    }

    #[test]
    fn scan_events_render_tick_and_work_item_identity() {
        let identity = identity();
        let summary = render_scan_summary_event(&ScanSummaryEvent {
            tick_id: Some("tick-1"),
            worker_kind: "role",
            worker: "multi-role:engineer",
            repo: &RepositoryId::new("forgejo:acme/service"),
            workflow_id: "reference-delivery",
            role: Some("engineer"),
            work_item_count: 1,
        });
        assert!(summary.contains(r#""event":"scan_summary""#));
        assert!(summary.contains(r#""tick_id":"tick-1""#));
        assert!(summary.contains(r#""work_item_count":1"#));

        let selected = render_work_item_selected_event(&WorkItemSelectedEvent {
            identity: &identity,
            workflow_id: "reference-delivery",
            worker: "multi-role:engineer",
        });
        assert!(selected.contains(r#""event":"work_item_selected""#));
        assert!(selected.contains(r#""decision_id":"decision/"#));
        assert!(selected.contains(r#""queue":"ready""#));
    }

    #[test]
    fn request_and_dispatch_events_render_safe_identifiers() {
        let identity = identity();
        let request = render_role_decision_request_event(&RoleDecisionRequestEvent {
            identity: &identity,
            workflow_id: "reference-delivery",
            authorized_actions: &["claim_code".to_string(), "nope".to_string()],
            available_external_tools: &["coding_workspace".to_string()],
        });
        assert!(request.contains(r#""event":"role_decision_request""#));
        assert!(request.contains(r#""workflow_id":"reference-delivery""#));
        assert!(request.contains(r#""authorized_action_count":2"#));
        assert!(request.contains(r#""authorized_actions":["claim_code","nope"]"#));
        assert!(request.contains(r#""available_external_tools":["coding_workspace"]"#));

        let dispatch = render_action_dispatch_event(&ActionDispatchEvent {
            identity: &identity,
            selected_action: "claim_code",
            transition: &TransitionId::new("claim_code"),
            external_executor_required: true,
            external_executor_id: Some("coding_workspace"),
            external_executor_available: Some(false),
            outcome: "no_op",
            no_op_reason: Some("required_executor_unavailable"),
        });
        assert!(dispatch.contains(r#""event":"action_dispatch""#));
        assert!(dispatch.contains(r#""transition":"claim_code""#));
        assert!(dispatch.contains(r#""external_executor_available":false"#));
        assert!(dispatch.contains(r#""no_op_reason":"required_executor_unavailable""#));
    }

    #[test]
    fn decision_events_render_safe_bounded_json() {
        let identity = identity();
        let rendered = render_role_decision_reply_event(&RoleDecisionReplyEvent {
            identity: &identity,
            selected_action: Some("claim_code"),
            validation_outcome: "valid",
            action_kind: "authorized_action",
            reason: Some("Authorization: bearer secret-token"),
            latency: Duration::from_millis(12),
            error: None,
        });

        assert!(rendered.contains(r#""event":"role_decision_reply""#));
        assert!(rendered.contains(r#""tick_id":"tick-1""#));
        assert!(rendered.contains(r#""latency_ms":12"#));
        assert!(rendered.contains(r#""reason_preview":"<redacted>""#));
        assert!(!rendered.contains("secret-token"));
    }

    #[test]
    fn transition_event_summarizes_effects_without_bodies() {
        let identity = identity();
        let effects = vec![
            WorkflowEffect::RemoveLabel(LabelId::new("ready")),
            WorkflowEffect::CreateComment {
                body: "full operator-facing comment body".to_string(),
            },
            WorkflowEffect::CreatePullRequest {
                correlation_key: Some("pr-correlation-key".to_string()),
            },
        ];
        let rendered = render_transition_execution_event(&TransitionExecutionEvent {
            identity: &identity,
            transition: &TransitionId::new("claim_code"),
            outcome: "mutated",
            stale_work: false,
            effects: &effects,
            failure_class: None,
            diagnostic_classes: Vec::new(),
            postcondition_outcome: "passed",
        });

        assert!(rendered.contains(
            r#""effects":["remove_label:ready","create_comment","create_pull_request"]"#
        ));
        assert!(!rendered.contains("full operator-facing"));
        assert!(!rendered.contains("pr-correlation-key"));
    }

    #[test]
    fn execution_error_classes_are_compact() {
        let error = ExecutionError::PostconditionFailed {
            postcondition: Postcondition::LabelPresent(LabelId::new("done")),
        };

        assert_eq!(
            execution_error_failure_class(&error),
            "postcondition_failed"
        );
        assert_eq!(postcondition_outcome_for_error(&error), "failed");
    }

    #[test]
    fn mechanical_reconciliation_event_names_zero_dependency_blocker() {
        let finding = ReconcileFinding::BlockedWithoutDependencies {
            target: ArtifactSource::Issue {
                number: ItemNumber::new(1),
            },
            transition: TransitionId::new("mark_code_ready"),
            dependency_count: 0,
            relation_count: 0,
        };
        let action = RecoveryAction::Diagnose {
            target: ArtifactSource::Issue {
                number: ItemNumber::new(1),
            },
            message: "blocked_artifact_without_dependencies".to_string(),
        };
        let rendered = render_mechanical_reconciliation_event(&MechanicalReconciliationEvent {
            worker: "mechanical",
            repo: &RepositoryId::new("forgejo:acme/service"),
            finding: &finding,
            action: &action,
        });

        assert!(rendered.contains(r#""event":"mechanical_reconciliation""#));
        assert!(rendered.contains(r#""finding":"blocked_without_dependencies""#));
        assert!(rendered.contains(r#""diagnostic":"blocked_artifact_without_dependencies""#));
        assert!(rendered.contains(r#""dependency_count":0"#));
        assert!(rendered.contains(r#""relation_count":0"#));
        assert!(rendered.contains(r#""transition":"mark_code_ready""#));
    }
}
