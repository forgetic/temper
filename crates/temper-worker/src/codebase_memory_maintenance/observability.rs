//! Aggregate-only lifecycle evidence for codebase-memory maintenance.

use crate::codebase_memory_retention::{
    CodebaseMemoryRetentionOutcome, CodebaseMemoryRetentionReport,
};

pub(super) fn emit_report(report: &CodebaseMemoryRetentionReport) {
    let discovery_outcome = if report.inventory_complete {
        "success"
    } else if report.outcome == CodebaseMemoryRetentionOutcome::TimedOut {
        "timeout"
    } else if report.inventory_attempted {
        "failure"
    } else {
        "skipped"
    };
    let timed_out = report.outcome == CodebaseMemoryRetentionOutcome::TimedOut;
    let record_count = count(report.inventory_record_count);
    let cache_bytes_available = report.cache_bytes.is_some();
    let cache_bytes = report.cache_bytes.unwrap_or_default();
    let failure_category = safe_failure_category(report.outcome);
    if report.inventory_complete {
        tracing::debug!(
            target: "temper::worker",
            service = "worker",
            event = "codebase_memory.maintenance.discovery.completed",
            discovery.method = "list_projects",
            discovery.inventory = "maintenance",
            discovery.targeted = false,
            duration_ms = report.inventory_duration_ms,
            outcome = discovery_outcome,
            timed_out,
            record_count,
            cache.bytes_available = cache_bytes_available,
            cache.bytes = cache_bytes,
            failure.category = failure_category,
            failure.message = safe_failure_message(report.outcome),
            "worker:  codebase-memory maintenance discovery completed",
        );
    } else {
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "codebase_memory.maintenance.discovery.completed",
            discovery.method = "list_projects",
            discovery.inventory = "maintenance",
            discovery.targeted = false,
            duration_ms = report.inventory_duration_ms,
            outcome = discovery_outcome,
            timed_out,
            record_count,
            cache.bytes_available = cache_bytes_available,
            cache.bytes = cache_bytes,
            failure.category = failure_category,
            failure.message = safe_failure_message(report.outcome),
            "worker:  codebase-memory maintenance discovery did not complete",
        );
    }

    let policy = report.policy.unwrap_or_default();
    let deleted_bytes_available = report.deleted_estimated_bytes.is_some();
    let deleted_bytes = report.deleted_estimated_bytes.unwrap_or_default();
    let outcome = report.outcome.as_str();
    let preserved_count = count(report.preserved.len());
    let candidate_count = count(report.candidates.len());
    let deletion_count = count(report.deleted.len());
    let failure_count = count(report.failed.len());
    let warn = matches!(
        report.outcome,
        CodebaseMemoryRetentionOutcome::PartialFailure
            | CodebaseMemoryRetentionOutcome::TimedOut
            | CodebaseMemoryRetentionOutcome::DiscoveryFailed
            | CodebaseMemoryRetentionOutcome::InventoryUncertain
    );
    if warn {
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "codebase_memory.retention.completed",
            outcome,
            duration_ms = report.duration_ms,
            retention.enabled = policy.enabled,
            retention.max_obsolete_projects = u64::from(policy.max_obsolete_projects),
            retention.max_age_days = u64::from(policy.max_age_days),
            retention.max_deletions_per_run = u64::from(policy.max_deletions_per_run),
            retention.maintenance_timeout_secs = policy.maintenance_timeout_secs,
            retention.preserved_count = preserved_count,
            retention.candidate_count = candidate_count,
            retention.deletion_count = deletion_count,
            retention.deleted_bytes_available = deleted_bytes_available,
            retention.deleted_estimated_bytes = deleted_bytes,
            retention.dry_run = report.dry_run,
            failure.count = failure_count,
            failure.category = failure_category,
            failure.message = safe_failure_message(report.outcome),
            "worker:  codebase-memory retention completed with operator evidence",
        );
    } else {
        tracing::info!(
            target: "temper::worker",
            service = "worker",
            event = "codebase_memory.retention.completed",
            outcome,
            duration_ms = report.duration_ms,
            retention.enabled = policy.enabled,
            retention.max_obsolete_projects = u64::from(policy.max_obsolete_projects),
            retention.max_age_days = u64::from(policy.max_age_days),
            retention.max_deletions_per_run = u64::from(policy.max_deletions_per_run),
            retention.maintenance_timeout_secs = policy.maintenance_timeout_secs,
            retention.preserved_count = preserved_count,
            retention.candidate_count = candidate_count,
            retention.deletion_count = deletion_count,
            retention.deleted_bytes_available = deleted_bytes_available,
            retention.deleted_estimated_bytes = deleted_bytes,
            retention.dry_run = report.dry_run,
            failure.count = failure_count,
            failure.category = failure_category,
            failure.message = safe_failure_message(report.outcome),
            "worker:  codebase-memory retention completed",
        );
    }
}

fn safe_failure_category(outcome: CodebaseMemoryRetentionOutcome) -> &'static str {
    match outcome {
        CodebaseMemoryRetentionOutcome::PartialFailure => "deletion_failure",
        CodebaseMemoryRetentionOutcome::TimedOut => "timeout",
        CodebaseMemoryRetentionOutcome::DiscoveryFailed => "provider_error",
        CodebaseMemoryRetentionOutcome::InventoryUncertain => "inventory_uncertain",
        CodebaseMemoryRetentionOutcome::SuppressedActiveWork => "active_work",
        CodebaseMemoryRetentionOutcome::SuppressedOverlap => "overlap",
        CodebaseMemoryRetentionOutcome::SafetyNoOp => "safety_no_op",
        CodebaseMemoryRetentionOutcome::Disabled | CodebaseMemoryRetentionOutcome::Completed => "",
    }
}

fn safe_failure_message(outcome: CodebaseMemoryRetentionOutcome) -> &'static str {
    match outcome {
        CodebaseMemoryRetentionOutcome::PartialFailure => "one or more deletions failed",
        CodebaseMemoryRetentionOutcome::TimedOut => "maintenance timed out",
        CodebaseMemoryRetentionOutcome::DiscoveryFailed => "provider discovery failed",
        CodebaseMemoryRetentionOutcome::InventoryUncertain => "provider inventory was uncertain",
        CodebaseMemoryRetentionOutcome::SuppressedActiveWork => {
            "active work suppressed maintenance"
        }
        CodebaseMemoryRetentionOutcome::SuppressedOverlap => "another maintenance pass was active",
        CodebaseMemoryRetentionOutcome::SafetyNoOp => "maintenance failed closed",
        CodebaseMemoryRetentionOutcome::Disabled | CodebaseMemoryRetentionOutcome::Completed => "",
    }
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
