//! Structured durable-assignment convergence events.

use temper_forge::RepositoryId;

use super::{AssignmentConvergenceError, AssignmentConvergenceOutcome};
use crate::classify::ArtifactSource;
use crate::metadata::DurableAssignment;

pub(super) fn emit_assignment_convergence(
    repo: &RepositoryId,
    target: ArtifactSource,
    expected: &DurableAssignment,
    result: &Result<AssignmentConvergenceOutcome, AssignmentConvergenceError>,
) {
    let (artifact_kind, artifact_number) = match target {
        ArtifactSource::Issue { number } => ("issue", number.get()),
        ArtifactSource::PullRequest { number } => ("pull_request", number.get()),
    };
    let job_id = expected.job_id.as_deref().unwrap_or("unknown");
    let attempt_id = expected.attempt_id.as_deref().unwrap_or("legacy");
    let worker_id = expected.worker_id.as_deref().unwrap_or("unknown");
    match result {
        Ok(
            outcome @ (AssignmentConvergenceOutcome::Converged
            | AssignmentConvergenceOutcome::AdvancedHeadRecovered
            | AssignmentConvergenceOutcome::Stale),
        ) => tracing::debug!(
            target: "temper::worker",
            service = "worker",
            event = "assignment.convergence",
            repo = repo.as_str(),
            artifact_kind,
            artifact_number,
            worker_id,
            job_id,
            attempt_id,
            convergence_result = convergence_outcome(*outcome),
            claim_converged = !matches!(outcome, AssignmentConvergenceOutcome::Stale),
            "durable assignment convergence completed"
        ),
        Ok(AssignmentConvergenceOutcome::Quarantined) => tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "assignment.convergence",
            repo = repo.as_str(),
            artifact_kind,
            artifact_number,
            worker_id,
            job_id,
            attempt_id,
            convergence_result = "quarantined",
            claim_converged = true,
            "durable assignment was quarantined for operator inspection"
        ),
        Err(error) => tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "assignment.convergence",
            repo = repo.as_str(),
            artifact_kind,
            artifact_number,
            worker_id,
            job_id,
            attempt_id,
            convergence_result = "unreconciled",
            claim_converged = false,
            error_kind = convergence_error_kind(error),
            "durable assignment remains unreconciled"
        ),
    }
}

const fn convergence_outcome(outcome: AssignmentConvergenceOutcome) -> &'static str {
    match outcome {
        AssignmentConvergenceOutcome::Converged => "converged",
        AssignmentConvergenceOutcome::AdvancedHeadRecovered => "advanced_head_recovered",
        AssignmentConvergenceOutcome::Stale => "stale",
        AssignmentConvergenceOutcome::Quarantined => "quarantined",
    }
}

const fn convergence_error_kind(error: &AssignmentConvergenceError) -> &'static str {
    match error {
        AssignmentConvergenceError::Forge(_) => "forge",
        AssignmentConvergenceError::Lease(_) => "lease",
        AssignmentConvergenceError::InvalidContract(_) => "invalid_contract",
    }
}
