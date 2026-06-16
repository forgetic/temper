//! Body-free classification of runner execution effects and errors.
//!
//! These helpers produce the compact, secret-free strings (`failure_class`,
//! `diagnostic_classes`, effect names) that the new `temper_log::emit::*` debug
//! and info sites carry as fields. The JSON-string event renderers that used to
//! live here are gone; only the classification logic survives because the debug
//! paths still need it.

mod classify;

use temper_workflow::WorkflowEffect;

pub use classify::{
    execution_error_diagnostic_classes, execution_error_failure_class,
    postcondition_outcome_for_error,
};

/// Returns compact, body-free descriptions of planned/applied effects.
///
/// Each effect collapses to a stable token (`add_label:ready`, `create_comment`,
/// `merge_pull_request`, …) with no operator-authored bodies, so the strings are
/// safe to log at debug as structured fields.
pub fn workflow_effect_summary(effects: &[WorkflowEffect]) -> Vec<String> {
    effects.iter().map(summarize_effect).collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use temper_workflow::{ExecutionError, LabelId, Postcondition, WorkflowEffect};

    #[test]
    fn effects_summarize_without_bodies() {
        let effects = vec![
            WorkflowEffect::RemoveLabel(LabelId::new("ready")),
            WorkflowEffect::CreateComment {
                body: "full operator-facing comment body".to_string(),
            },
            WorkflowEffect::CreatePullRequest {
                correlation_key: Some("pr-correlation-key".to_string()),
            },
        ];
        let summary = workflow_effect_summary(&effects);
        assert_eq!(
            summary,
            vec![
                "remove_label:ready",
                "create_comment",
                "create_pull_request"
            ]
        );
        assert!(!summary.iter().any(|line| line.contains("operator-facing")));
        assert!(
            !summary
                .iter()
                .any(|line| line.contains("pr-correlation-key"))
        );
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
