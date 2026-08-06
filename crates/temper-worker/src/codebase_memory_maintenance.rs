//! Host-controlled codebase-memory provider maintenance.
//!
//! The adapter negotiates a paginated inventory plus `delete_project` directly
//! with the provider. Neither destructive operation is registered with an agent
//! tool registry or included in model-visible schemas.

use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::codebase_memory_retention::{
    CodebaseMemoryRetentionOutcome, CodebaseMemoryRetentionReport, CodebaseMemoryRetentionScope,
    maintain_obsolete_codebase_memory_indexes_until,
};
use crate::run::WorkerActivityProbe;
use fs2::FileExt;
use temper_protocol_agent::CodebaseMemoryRetentionPolicy;

mod provider;
use provider::{ProviderSession, unix_time_secs};

const MAINTENANCE_LOCK_FILE: &str = ".temper-codebase-memory-maintenance.lock";

struct MaintenanceLockFailure {
    reason: String,
    outcome: CodebaseMemoryRetentionOutcome,
}

/// Complete non-secret input needed by explicit or periodic maintenance.
#[derive(Clone, Debug)]
pub struct CodebaseMemoryMaintenanceConfig {
    command: String,
    args: Vec<String>,
    startup_timeout: Duration,
    pub policy: CodebaseMemoryRetentionPolicy,
    pub scope: CodebaseMemoryRetentionScope,
}

impl CodebaseMemoryMaintenanceConfig {
    pub fn new(
        command: impl Into<String>,
        args: Vec<String>,
        startup_timeout: Duration,
        policy: CodebaseMemoryRetentionPolicy,
        scope: CodebaseMemoryRetentionScope,
    ) -> Self {
        Self {
            command: command.into(),
            args,
            startup_timeout,
            policy,
            scope,
        }
    }
}

/// Explicit host maintenance entry point. It shares the same overlap lock,
/// provider negotiation, safety classifier, and bounds as periodic workers.
pub fn run_codebase_memory_maintenance(
    config: &CodebaseMemoryMaintenanceConfig,
    active_work: &dyn Fn() -> bool,
) -> CodebaseMemoryRetentionReport {
    let started = Instant::now();
    let mut report = run_codebase_memory_maintenance_inner(config, active_work);
    report.policy = Some(config.policy);
    report.duration_ms = duration_ms(started.elapsed());
    emit_report(&report);
    report
}

fn run_codebase_memory_maintenance_inner(
    config: &CodebaseMemoryMaintenanceConfig,
    active_work: &dyn Fn() -> bool,
) -> CodebaseMemoryRetentionReport {
    if !config.policy.enabled {
        return CodebaseMemoryRetentionReport::no_op_with_outcome(
            "retention policy is disabled",
            CodebaseMemoryRetentionOutcome::Disabled,
        );
    }
    if active_work() {
        return CodebaseMemoryRetentionReport::no_op_with_outcome(
            "active worker assignments suppress provider maintenance",
            CodebaseMemoryRetentionOutcome::SuppressedActiveWork,
        );
    }
    let lock = match maintenance_lock(&config.scope.workspace_root) {
        Ok(lock) => lock,
        Err(failure) => {
            return CodebaseMemoryRetentionReport::no_op_with_outcome(
                failure.reason,
                failure.outcome,
            );
        }
    };
    let _lock = lock;
    if active_work() {
        return CodebaseMemoryRetentionReport::no_op_with_outcome(
            "active worker assignments appeared before provider discovery",
            CodebaseMemoryRetentionOutcome::SuppressedActiveWork,
        );
    }

    let deadline = Instant::now() + Duration::from_secs(config.policy.maintenance_timeout_secs);
    let timeout = config
        .startup_timeout
        .min(deadline.saturating_duration_since(Instant::now()));
    if timeout.is_zero() {
        return CodebaseMemoryRetentionReport::no_op_with_outcome(
            "maintenance deadline expired before provider negotiation",
            CodebaseMemoryRetentionOutcome::TimedOut,
        );
    }
    let mut provider = match ProviderSession::connect(&config.command, &config.args, timeout) {
        Ok(provider) => provider,
        Err(error) => {
            return CodebaseMemoryRetentionReport::no_op_with_outcome(
                format!("provider maintenance API was not safely negotiated: {error}"),
                CodebaseMemoryRetentionOutcome::DiscoveryFailed,
            );
        }
    };
    maintain_obsolete_codebase_memory_indexes_until(
        &mut provider,
        config.policy,
        &config.scope,
        unix_time_secs(),
        active_work,
        deadline,
    )
}

fn maintenance_lock(workspace_root: &PathBuf) -> Result<File, MaintenanceLockFailure> {
    std::fs::create_dir_all(workspace_root).map_err(|error| MaintenanceLockFailure {
        reason: format!("workspace root is unavailable for maintenance locking: {error}"),
        outcome: CodebaseMemoryRetentionOutcome::SafetyNoOp,
    })?;
    let path = workspace_root.join(MAINTENANCE_LOCK_FILE);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| MaintenanceLockFailure {
            reason: format!("open maintenance overlap lock failed: {error}"),
            outcome: CodebaseMemoryRetentionOutcome::SafetyNoOp,
        })?;
    lock.try_lock_exclusive()
        .map_err(|_| MaintenanceLockFailure {
            reason: "another codebase-memory maintenance pass owns the overlap lock".to_string(),
            outcome: CodebaseMemoryRetentionOutcome::SuppressedOverlap,
        })?;
    Ok(lock)
}

/// Joinable worker-owned periodic maintenance thread. A dedicated thread keeps
/// blocking provider stdio and interval waits off the async worker runtime.
pub struct CodebaseMemoryMaintenanceTask {
    stop: Arc<(Mutex<bool>, Condvar)>,
    joined: Option<JoinHandle<()>>,
}

impl CodebaseMemoryMaintenanceTask {
    pub fn stop(mut self) {
        self.request_stop();
        if let Some(joined) = self.joined.take() {
            let _ = joined.join();
        }
    }

    fn request_stop(&self) {
        let (stopped, wake) = &*self.stop;
        *stopped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        wake.notify_all();
    }
}

impl Drop for CodebaseMemoryMaintenanceTask {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(joined) = self.joined.take() {
            let _ = joined.join();
        }
    }
}

/// Starts immediate-then-periodic maintenance for split and standalone worker
/// composition roots. Disabled policies do not create a thread.
pub fn spawn_codebase_memory_maintenance_task(
    config: Option<CodebaseMemoryMaintenanceConfig>,
    activity: WorkerActivityProbe,
) -> Option<CodebaseMemoryMaintenanceTask> {
    let config = config.filter(|config| config.policy.enabled)?;
    let stop = Arc::new((Mutex::new(false), Condvar::new()));
    let thread_stop = Arc::clone(&stop);
    let joined = thread::Builder::new()
        .name("temper-codebase-memory-maintenance".to_string())
        .spawn(move || {
            loop {
                if stopped(&thread_stop) {
                    break;
                }
                let _report =
                    run_codebase_memory_maintenance(&config, &|| activity.has_active_work());
                let (stopped, wake) = &*thread_stop;
                let stopped = stopped
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if *stopped {
                    break;
                }
                let (stopped, _) = wake
                    .wait_timeout(
                        stopped,
                        Duration::from_secs(config.policy.maintenance_interval_secs),
                    )
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if *stopped {
                    break;
                }
            }
        })
        .ok()?;
    Some(CodebaseMemoryMaintenanceTask {
        stop,
        joined: Some(joined),
    })
}

fn stopped(stop: &Arc<(Mutex<bool>, Condvar)>) -> bool {
    *stop
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn emit_report(report: &CodebaseMemoryRetentionReport) {
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

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "codebase_memory_maintenance/observability_tests.rs"]
mod observability_tests;
