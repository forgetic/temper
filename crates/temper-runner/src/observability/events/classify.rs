//! Compact, body-free classification of execution errors for events.

use temper_workflow::{ExecutionError, PlanDiagnostic};

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

pub(super) fn plan_diagnostic_class(diagnostic: &PlanDiagnostic) -> &'static str {
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
