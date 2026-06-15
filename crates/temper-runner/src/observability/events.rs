//! Render-input structs and renderers for runner observability events.

mod classify;
mod render;

use std::time::Duration;

use temper_forge::RepositoryId;
use temper_workflow::{ReconcileFinding, RecoveryAction, TransitionId, WorkflowEffect};

use super::WorkItemIdentity;

pub use classify::{
    execution_error_diagnostic_classes, execution_error_failure_class,
    postcondition_outcome_for_error,
};
pub use render::{
    render_action_dispatch_event, render_mechanical_reconciliation_event,
    render_role_decision_reply_event, render_role_decision_request_event,
    render_scan_summary_event, render_transition_execution_event, render_work_item_selected_event,
    workflow_effect_summary,
};

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

#[cfg(test)]
mod tests {
    use super::*;
    use temper_forge::ItemNumber;
    use temper_workflow::{
        ArtifactKindId, ArtifactSource, ExecutionError, LabelId, Postcondition, QueueId, RoleId,
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
