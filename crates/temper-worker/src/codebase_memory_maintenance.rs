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

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::codebase_memory_retention::{
    CodebaseMemoryRetentionOutcome, CodebaseMemoryRetentionReport, CodebaseMemoryRetentionScope,
    apply_verified_codebase_memory_plan, maintain_obsolete_codebase_memory_indexes_until,
    plan_obsolete_codebase_memory_indexes_until,
};
use crate::run::WorkerActivityProbe;
use fs2::FileExt;
use temper_protocol_agent::CodebaseMemoryRetentionPolicy;

mod observability;
mod provider;
use observability::emit_report;
use provider::{ProviderSession, unix_time_secs};

const MAINTENANCE_LOCK_FILE: &str = ".temper-codebase-memory-maintenance.lock";
const PROVIDER_ID_PREFIX: &str = "forgejo:";

/// Explicit operator recovery mode. Dry-run is the CLI default.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodebaseMemoryRecoveryMode {
    DryRun,
    Apply,
}

/// Optional stable logical project verification/rebuild request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodebaseMemoryRecoveryTarget {
    pub logical_repository: String,
    pub provider_key: String,
    pub rebuild_from: Option<PathBuf>,
}

/// Verified provider identity, independent from the cache instance identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodebaseMemoryProviderIdentity {
    pub name: String,
    pub version: String,
    pub cache_instance_id: Option<String>,
}

/// Targeted stable-project readiness and safe-probe evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodebaseMemoryStableProjectReport {
    pub logical_repository: String,
    pub provider_key: String,
    pub status: Option<String>,
    pub ready: bool,
    pub rebuild_requested: bool,
    pub rebuild_completed: bool,
    pub safe_probe_succeeded: bool,
    pub lookup_latency_ms: Option<u64>,
    pub failure: Option<String>,
}

/// Structured operator report. It contains no provider response bodies or
/// configuration secrets and can be rendered directly as bounded JSON.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodebaseMemoryRecoveryReport {
    pub mode: CodebaseMemoryRecoveryMode,
    pub provider: Option<CodebaseMemoryProviderIdentity>,
    pub configured_bounds: CodebaseMemoryRetentionPolicy,
    /// Fingerprint binding an apply invocation to reviewed dry-run evidence.
    pub plan_id: Option<String>,
    pub preflight_verified: bool,
    pub retention: CodebaseMemoryRetentionReport,
    pub stable_project: Option<CodebaseMemoryStableProjectReport>,
    pub failure: Option<String>,
}

impl CodebaseMemoryRecoveryReport {
    pub fn succeeded(&self) -> bool {
        self.failure.is_none()
            && self
                .provider
                .as_ref()
                .is_some_and(|provider| provider.cache_instance_id.is_some())
            && self.retention.inventory_complete
            && self.retention.no_op_reason.is_none()
            && self.retention.failed.is_empty()
            && self.stable_project.as_ref().is_none_or(|project| {
                project.failure.is_none() && project.ready && project.safe_probe_succeeded
            })
    }
}

/// Derives the same stable provider key used by agent indexing from a durable
/// Forge repository identity and logical owner/name.
pub fn codebase_memory_provider_key(repository_id: &str, owner: &str, name: &str) -> String {
    let mut digest = Sha256::new();
    for (label, value) in [
        (b"id".as_slice(), repository_id.as_bytes()),
        (b"owner".as_slice(), owner.as_bytes()),
        (b"name".as_slice(), name.as_bytes()),
    ] {
        digest.update((label.len() as u64).to_be_bytes());
        digest.update(label);
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    format!("temper-v1-{:x}", digest.finalize())
}

/// Builds a stable target from a configured `owner/name`; no checkout path is
/// consulted when deriving identity.
pub fn codebase_memory_recovery_target(
    logical_repository: &str,
    rebuild_from: Option<PathBuf>,
) -> Result<CodebaseMemoryRecoveryTarget, String> {
    let mut parts = logical_repository.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err("repository must be a configured `owner/name`".to_string());
    };
    if owner.is_empty() || name.is_empty() {
        return Err("repository must be a configured `owner/name`".to_string());
    }
    let repository_id = format!("{PROVIDER_ID_PREFIX}{owner}/{name}");
    Ok(CodebaseMemoryRecoveryTarget {
        logical_repository: logical_repository.to_string(),
        provider_key: codebase_memory_provider_key(&repository_id, owner, name),
        rebuild_from,
    })
}

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
    index_timeout: Duration,
    pub policy: CodebaseMemoryRetentionPolicy,
    pub scope: CodebaseMemoryRetentionScope,
}

impl CodebaseMemoryMaintenanceConfig {
    pub fn new(
        command: impl Into<String>,
        args: Vec<String>,
        startup_timeout: Duration,
        index_timeout: Duration,
        policy: CodebaseMemoryRetentionPolicy,
        scope: CodebaseMemoryRetentionScope,
    ) -> Self {
        Self {
            command: command.into(),
            args,
            startup_timeout,
            index_timeout,
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

/// Dry-run-first host recovery entry point. Apply requires the reviewed dry-run
/// fingerprint, performs a second complete inventory, and requires it to exactly
/// match before invoking deletion for only the verified proposed identities.
pub fn run_codebase_memory_recovery(
    config: &CodebaseMemoryMaintenanceConfig,
    mode: CodebaseMemoryRecoveryMode,
    expected_plan_id: Option<&str>,
    mut target: Option<CodebaseMemoryRecoveryTarget>,
    active_work: &dyn Fn() -> bool,
) -> CodebaseMemoryRecoveryReport {
    let mut report = CodebaseMemoryRecoveryReport {
        mode,
        provider: None,
        configured_bounds: config.policy,
        plan_id: None,
        preflight_verified: false,
        retention: CodebaseMemoryRetentionReport::default(),
        stable_project: None,
        failure: None,
    };
    if !config.policy.enabled {
        report.retention = CodebaseMemoryRetentionReport::no_op("retention policy is disabled");
        report.failure = Some("configured codebase-memory retention is disabled".to_string());
        return report;
    }
    if active_work() {
        report.retention = CodebaseMemoryRetentionReport::no_op(
            "active worker assignments suppress provider maintenance",
        );
        report.failure = report.retention.no_op_reason.clone();
        return report;
    }
    let lock = match maintenance_lock(&config.scope.workspace_root) {
        Ok(lock) => lock,
        Err(failure) => {
            report.retention = CodebaseMemoryRetentionReport::no_op_with_outcome(
                failure.reason.clone(),
                failure.outcome,
            );
            report.failure = Some(failure.reason);
            return report;
        }
    };
    let _lock = lock;
    if active_work() {
        let reason = "active worker assignments appeared before provider discovery".to_string();
        report.retention = CodebaseMemoryRetentionReport::no_op(reason.clone());
        report.failure = Some(reason);
        return report;
    }

    let deadline = Instant::now() + Duration::from_secs(config.policy.maintenance_timeout_secs);
    let timeout = config
        .startup_timeout
        .min(deadline.saturating_duration_since(Instant::now()));
    if timeout.is_zero() {
        let reason = "maintenance deadline expired before provider negotiation".to_string();
        report.retention = CodebaseMemoryRetentionReport::no_op(reason.clone());
        report.failure = Some(reason);
        return report;
    }
    let mut provider = match ProviderSession::connect(&config.command, &config.args, timeout) {
        Ok(provider) => provider,
        Err(error) => {
            let reason = format!("provider maintenance API was not safely negotiated: {error}");
            report.retention = CodebaseMemoryRetentionReport::no_op(reason.clone());
            report.failure = Some(reason);
            return report;
        }
    };
    let (provider_name, provider_version) = provider.identity();
    report.provider = Some(CodebaseMemoryProviderIdentity {
        name: provider_name.to_string(),
        version: provider_version.to_string(),
        cache_instance_id: None,
    });

    let now = unix_time_secs();
    let review = plan_obsolete_codebase_memory_indexes_until(
        &mut provider,
        config.policy,
        &config.scope,
        now,
        active_work,
        deadline,
    );
    report.plan_id = Some(retention_plan_id(
        config.policy,
        report
            .provider
            .as_ref()
            .expect("provider identity was recorded after negotiation"),
        target.as_ref(),
        &review,
    ));
    report.retention = match mode {
        CodebaseMemoryRecoveryMode::DryRun => review,
        CodebaseMemoryRecoveryMode::Apply => {
            if expected_plan_id != report.plan_id.as_deref() {
                // A rejected apply is not an alternate way to mint review
                // evidence; only an explicit dry-run emits a reusable plan.
                report.plan_id = None;
                let mut unconfirmed = review;
                unconfirmed.no_op_reason = Some(
                    "destructive execution refused because --plan does not match the current verified dry-run"
                        .to_string(),
                );
                unconfirmed.proposed.clear();
                unconfirmed
            } else if !review.inventory_complete
                || review.cache_instance_id.is_none()
                || review.no_op_reason.is_some()
            {
                review
            } else if let Err(reason) = preflight_recovery_target(&provider, target.as_mut()) {
                let mut refused = review;
                refused.no_op_reason = Some(format!(
                    "stable-project preflight failed before destructive execution: {reason}"
                ));
                refused.proposed.clear();
                refused
            } else {
                let preflight = plan_obsolete_codebase_memory_indexes_until(
                    &mut provider,
                    config.policy,
                    &config.scope,
                    now,
                    active_work,
                    deadline,
                );
                let (unchanged, mut preflight) = verify_unchanged_preflight(&review, preflight);
                if !unchanged {
                    preflight
                } else if let Err(reason) =
                    verify_candidate_quiescence(&mut provider, &preflight, deadline)
                {
                    preflight.no_op_reason = Some(reason);
                    preflight.proposed.clear();
                    preflight
                } else {
                    report.preflight_verified = true;
                    apply_verified_codebase_memory_plan(
                        &mut provider,
                        preflight,
                        active_work,
                        deadline,
                    )
                }
            }
        }
    };
    if let Some(identity) = report.provider.as_mut() {
        identity.cache_instance_id =
            if report.retention.inventory_complete && report.retention.no_op_reason.is_none() {
                report.retention.cache_instance_id.clone()
            } else {
                None
            };
    }
    if !report.retention.inventory_complete || report.retention.no_op_reason.is_some() {
        report.failure = report
            .retention
            .no_op_reason
            .clone()
            .or_else(|| Some("provider inventory was incomplete".to_string()));
        return report;
    }
    if !report.retention.failed.is_empty() {
        report.failure = Some("one or more verified provider deletions failed".to_string());
        return report;
    }

    if let Some(target) = target {
        report.stable_project = Some(run_stable_project_recovery(
            &mut provider,
            target,
            Instant::now() + config.index_timeout,
        ));
        if let Some(failure) = report
            .stable_project
            .as_ref()
            .and_then(|project| project.failure.clone())
        {
            report.failure = Some(failure);
        }
    }
    report
}

fn retention_plan_id(
    policy: CodebaseMemoryRetentionPolicy,
    provider: &CodebaseMemoryProviderIdentity,
    target: Option<&CodebaseMemoryRecoveryTarget>,
    report: &CodebaseMemoryRetentionReport,
) -> String {
    // Bind apply not only to the exact classification and cache instance, but
    // also to the negotiated provider and selected stable logical identity.
    // The explicit rebuild source is intentionally authorized by --apply; its
    // checkout path never participates in stable provider identity.
    let target_identity = target.map(|target| (&target.logical_repository, &target.provider_key));
    let stable_report = stable_retention_evidence(report);
    let encoded = serde_json::to_vec(&(policy, provider, target_identity, stable_report))
        .expect("non-secret retention plan always serializes");
    format!("sha256:{:x}", Sha256::digest(encoded))
}

fn preflight_recovery_target(
    provider: &ProviderSession,
    target: Option<&mut CodebaseMemoryRecoveryTarget>,
) -> Result<(), String> {
    let Some(target) = target else {
        return Ok(());
    };
    provider.validate_recovery_tools(target.rebuild_from.is_some())?;
    let Some(source) = target.rebuild_from.as_ref() else {
        return Ok(());
    };
    let canonical = source
        .canonicalize()
        .map_err(|error| format!("canonicalize explicit rebuild source failed: {error}"))?;
    if !canonical.is_dir() {
        return Err("explicit rebuild source is not a directory".to_string());
    }
    target.rebuild_from = Some(canonical);
    Ok(())
}

fn verify_unchanged_preflight(
    review: &CodebaseMemoryRetentionReport,
    mut preflight: CodebaseMemoryRetentionReport,
) -> (bool, CodebaseMemoryRetentionReport) {
    if stable_retention_evidence(&preflight) == stable_retention_evidence(review) {
        return (true, preflight);
    }
    preflight.no_op_reason = Some(
        "destructive execution refused because provider inventory changed after the dry-run"
            .to_string(),
    );
    preflight.proposed.clear();
    (false, preflight)
}

fn stable_retention_evidence(
    report: &CodebaseMemoryRetentionReport,
) -> CodebaseMemoryRetentionReport {
    let mut stable = report.clone();
    // Latency is operator evidence, not provider inventory identity.
    stable.inventory_duration_ms = 0;
    stable.duration_ms = 0;
    stable
}

fn verify_candidate_quiescence(
    provider: &mut ProviderSession,
    preflight: &CodebaseMemoryRetentionReport,
    deadline: Instant,
) -> Result<(), String> {
    provider
        .validate_status_tool()
        .map_err(|error| format!("candidate quiescence API was not safely negotiated: {error}"))?;
    for candidate in &preflight.proposed {
        let status = provider
            .index_status(&candidate.project, deadline)
            .map_err(|error| {
                format!(
                    "candidate `{}` quiescence could not be verified: {error}",
                    candidate.project
                )
            })?;
        if status.active {
            return Err(format!(
                "candidate `{}` is actively indexing; deletion refused",
                candidate.project
            ));
        }
        if matches!(
            status.status.as_str(),
            "missing" | "not_found" | "not-found" | "not_indexed" | "not-indexed"
        ) {
            return Err(format!(
                "candidate `{}` changed after inventory; deletion refused",
                candidate.project
            ));
        }
    }
    Ok(())
}

fn run_stable_project_recovery(
    provider: &mut ProviderSession,
    target: CodebaseMemoryRecoveryTarget,
    deadline: Instant,
) -> CodebaseMemoryStableProjectReport {
    let rebuild_requested = target.rebuild_from.is_some();
    let mut report = CodebaseMemoryStableProjectReport {
        logical_repository: target.logical_repository,
        provider_key: target.provider_key,
        status: None,
        ready: false,
        rebuild_requested,
        rebuild_completed: false,
        safe_probe_succeeded: false,
        lookup_latency_ms: None,
        failure: None,
    };
    if let Err(error) = provider.validate_recovery_tools(rebuild_requested) {
        report.failure = Some(format!(
            "stable-project recovery API was not safely negotiated: {error}"
        ));
        return report;
    }
    if let Some(source) = target.rebuild_from {
        let source = match source.canonicalize() {
            Ok(source) if source.is_dir() => source,
            Ok(_) => {
                report.failure = Some("explicit rebuild source is not a directory".to_string());
                return report;
            }
            Err(error) => {
                report.failure = Some(format!(
                    "canonicalize explicit rebuild source failed: {error}"
                ));
                return report;
            }
        };
        if let Err(error) = provider.rebuild_project(&report.provider_key, &source, deadline) {
            report.failure = Some(format!("stable project rebuild failed: {error}"));
            return report;
        }
        report.rebuild_completed = true;
    }

    loop {
        let lookup_started = Instant::now();
        match provider.index_status(&report.provider_key, deadline) {
            Ok(status) => {
                report.lookup_latency_ms =
                    Some(u64::try_from(lookup_started.elapsed().as_millis()).unwrap_or(u64::MAX));
                report.status = Some(status.status);
                report.ready = status.ready;
            }
            Err(error) => {
                report.failure = Some(format!("stable project status failed: {error}"));
                return report;
            }
        }
        if report.ready || !rebuild_requested {
            break;
        }
        let still_indexing = report.status.as_deref().is_some_and(|status| {
            matches!(
                status,
                "indexing" | "building" | "in_progress" | "in-progress" | "queued"
            )
        });
        if !still_indexing || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(
            Duration::from_millis(250).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    if !report.ready {
        report.failure = Some("stable project is not ready after recovery".to_string());
        return report;
    }
    match provider.safe_probe(&report.provider_key, deadline) {
        Ok(()) => report.safe_probe_succeeded = true,
        Err(error) => report.failure = Some(format!("stable project safe probe failed: {error}")),
    }
    report
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

#[cfg(test)]
#[path = "codebase_memory_maintenance/recovery_tests.rs"]
mod recovery_tests;

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "codebase_memory_maintenance/observability_tests.rs"]
mod observability_tests;
