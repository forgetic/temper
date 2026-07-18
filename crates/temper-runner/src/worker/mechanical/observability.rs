//! Structured measurements for mechanical phases and reconciliation reports.

use super::Progress;
use crate::observability::artifact_ref;
use crate::scan::ArtifactAddress;
use crate::worker::saturating_u64;
use std::future::Future;
use std::time::Instant;
use temper_forge::{Forge, RepositoryId};
use temper_log::strip_provider_scheme;
use temper_workflow::{ApplyOutcome, ReconciliationMode};

/// Runs one expensive mechanical phase and emits exactly one terminal debug
/// measurement. The optional backend counter is sampled around the phase so
/// Forgejo-backed runs expose a request delta without making observability part
/// of correctness.
pub(super) async fn measure_mechanical_phase<F, Fut, T, E>(
    forge: &F,
    repo: &RepositoryId,
    scope: &'static str,
    address: Option<ArtifactAddress>,
    phase: &'static str,
    future: Fut,
) -> Result<T, E>
where
    F: Forge + ?Sized,
    Fut: Future<Output = Result<T, E>>,
{
    let started = Instant::now();
    let provider_requests_before = forge.provider_request_count();
    let result = future.await;
    let provider_requests = provider_requests_before.and_then(|before| {
        forge
            .provider_request_count()
            .map(|after| after.saturating_sub(before))
    });
    let outcome = if result.is_ok() { "success" } else { "failed" };
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let provider_request_total = provider_requests.unwrap_or(0);
    let provider_requests_available = provider_requests.is_some();
    let repository = strip_provider_scheme(repo.as_str());
    if let Some(address) = address {
        let artifact = artifact_ref(repo, address.source()).to_string();
        tracing::debug!(
            target: "temper::worker",
            measurement = "mechanical.phase",
            repo = repository,
            mechanical.scope = scope,
            mechanical.phase = phase,
            artifact.ref = artifact,
            outcome,
            duration_ms,
            provider.request_total = provider_request_total,
            provider.requests_available = provider_requests_available,
            "mechanical {scope} {phase} {outcome}"
        );
    } else {
        tracing::debug!(
            target: "temper::worker",
            measurement = "mechanical.phase",
            repo = repository,
            mechanical.scope = scope,
            mechanical.phase = phase,
            outcome,
            duration_ms,
            provider.request_total = provider_request_total,
            provider.requests_available = provider_requests_available,
            "mechanical {scope} {phase} {outcome}"
        );
    }
    result
}

/// Logs mechanical recovery findings/actions at debug.
///
/// Reconciliation recovery is a §5 "between" cause (lease requeues, drift
/// repairs, advisory diagnoses), not a §7 workflow state change, so it stays at
/// debug under the worker target. The names are compact, body-free tokens.
pub(super) fn log_mechanical_reconciliation(
    worker: &str,
    repo: &RepositoryId,
    report: &temper_workflow::ReconcileReport,
) {
    for (finding, action) in report.findings.iter().zip(report.actions.iter()) {
        tracing::debug!(
            target: "temper::worker",
            worker_kind = "mechanical",
            worker,
            repo = repo.as_str(),
            finding = finding_name(finding),
            action = action_name(action),
            "reconcile: {} -> {}",
            finding_name(finding),
            action_name(action),
        );
    }
}

pub(super) fn log_mechanical_reconciliation_summary(
    worker: &str,
    repo: &RepositoryId,
    mode: ReconciliationMode,
    report: &temper_workflow::ReconcileReport,
    outcome: &ApplyOutcome,
    progress: Progress,
) {
    tracing::debug!(
        target: "temper::worker",
        worker_kind = "mechanical",
        worker,
        repo = repo.as_str(),
        mode = reconciliation_mode_name(mode),
        snapshot_count = saturating_u64(report.snapshot_count),
        cache_hits = report.cache_stats.hits,
        cache_misses = report.cache_stats.misses,
        cache_forced_refreshes = report.cache_stats.forced_refreshes,
        cache_invalidations = report.cache_stats.invalidations,
        cache_evictions = report.cache_stats.evictions,
        finding_count = saturating_u64(report.findings.len()),
        recovery_action_count = saturating_u64(report.actions.len()),
        applied_action_count = saturating_u64(outcome.applied.len()),
        advisory_action_count = saturating_u64(outcome.advisory.len()),
        changed = progress.changed,
        progress_actions = u64::from(progress.actions),
        "reconcile {} pass: {} finding(s), {} applied",
        reconciliation_mode_name(mode),
        report.findings.len(),
        outcome.applied.len(),
    );
}

fn finding_name(finding: &temper_workflow::ReconcileFinding) -> &'static str {
    use temper_workflow::ReconcileFinding;
    match finding {
        ReconcileFinding::ExpiredAssignment { .. } => "expired_assignment",
        ReconcileFinding::ExpiredLease { .. } => "expired_lease",
        ReconcileFinding::ImpossibleState { .. } => "impossible_state",
        ReconcileFinding::ClassificationDrift { .. } => "classification_drift",
        ReconcileFinding::BlockedWithoutDependencies { .. } => "blocked_without_dependencies",
        ReconcileFinding::PartialTransition { .. } => "partial_transition",
        ReconcileFinding::StaleCommand { .. } => "stale_command",
        ReconcileFinding::DependenciesResolved { .. } => "dependencies_resolved",
    }
}

fn action_name(action: &temper_workflow::RecoveryAction) -> &'static str {
    use temper_workflow::RecoveryAction;
    match action {
        RecoveryAction::ConvergeAssignment { .. } => "converge_assignment",
        RecoveryAction::RequeueLease { .. } => "requeue_lease",
        RecoveryAction::Escalate { .. } => "escalate",
        RecoveryAction::Repair { .. } => "repair",
        RecoveryAction::MarkReconciled { .. } => "mark_reconciled",
        RecoveryAction::Unblock { .. } => "unblock",
        RecoveryAction::Diagnose { .. } => "diagnose",
    }
}

fn reconciliation_mode_name(mode: ReconciliationMode) -> &'static str {
    match mode {
        ReconciliationMode::Bounded => "bounded",
        ReconciliationMode::DeepAudit => "deep-audit",
    }
}
