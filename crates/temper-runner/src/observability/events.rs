use std::time::Duration;

use temper_workflow::{ExecutionError, PlanDiagnostic, TransitionId, WorkflowEffect};

use super::{redacted_preview, StructuredEvent, WorkItemIdentity};

const PREVIEW_LIMIT: usize = 240;

/// Render input for a role-decision request event.
pub struct RoleDecisionRequestEvent<'a> {
    pub identity: &'a WorkItemIdentity,
    pub workflow_id: &'a str,
    pub authorized_actions: &'a [String],
    pub available_external_tools: &'a [String],
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
        ExecutionError::UnsupportedEffect { .. } => "unsupported_effect".to_string(),
        ExecutionError::UnresolvedAssignee { .. } => "unresolved_assignee".to_string(),
        ExecutionError::UnresolvedReviewer { .. } => "unresolved_reviewer".to_string(),
        ExecutionError::MissingCorrelationKey { .. } => "missing_correlation_key".to_string(),
        ExecutionError::UnresolvedPullRequestCreate { .. } => {
            "unresolved_pull_request_create".to_string()
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
}
