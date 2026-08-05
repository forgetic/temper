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
    CodebaseMemoryRetentionReport, CodebaseMemoryRetentionScope,
    maintain_obsolete_codebase_memory_indexes_until,
};
use crate::run::WorkerActivityProbe;
use fs2::FileExt;
use temper_protocol_agent::CodebaseMemoryRetentionPolicy;

mod provider;
use provider::{ProviderSession, unix_time_secs};

const MAINTENANCE_LOCK_FILE: &str = ".temper-codebase-memory-maintenance.lock";

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
    if !config.policy.enabled {
        return CodebaseMemoryRetentionReport::no_op("retention policy is disabled");
    }
    if active_work() {
        return CodebaseMemoryRetentionReport::no_op(
            "active worker assignments suppress provider maintenance",
        );
    }
    let lock = match maintenance_lock(&config.scope.workspace_root) {
        Ok(lock) => lock,
        Err(reason) => return CodebaseMemoryRetentionReport::no_op(reason),
    };
    let _lock = lock;
    if active_work() {
        return CodebaseMemoryRetentionReport::no_op(
            "active worker assignments appeared before provider discovery",
        );
    }

    let deadline = Instant::now() + Duration::from_secs(config.policy.maintenance_timeout_secs);
    let timeout = config
        .startup_timeout
        .min(deadline.saturating_duration_since(Instant::now()));
    if timeout.is_zero() {
        return CodebaseMemoryRetentionReport::no_op(
            "maintenance deadline expired before provider negotiation",
        );
    }
    let mut provider = match ProviderSession::connect(&config.command, &config.args, timeout) {
        Ok(provider) => provider,
        Err(error) => {
            return CodebaseMemoryRetentionReport::no_op(format!(
                "provider maintenance API was not safely negotiated: {error}"
            ));
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

fn maintenance_lock(workspace_root: &PathBuf) -> Result<File, String> {
    std::fs::create_dir_all(workspace_root).map_err(|error| {
        format!("workspace root is unavailable for maintenance locking: {error}")
    })?;
    let path = workspace_root.join(MAINTENANCE_LOCK_FILE);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("open maintenance overlap lock failed: {error}"))?;
    lock.try_lock_exclusive().map_err(|_| {
        "another codebase-memory maintenance pass owns the overlap lock".to_string()
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
                let report =
                    run_codebase_memory_maintenance(&config, &|| activity.has_active_work());
                emit_report(&report);
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
    let no_op = report.no_op_reason.as_deref().unwrap_or("");
    tracing::info!(
        target: "temper::worker",
        service = "worker",
        event = "codebase_memory.retention.completed",
        inventory_complete = report.inventory_complete,
        preserved = report.preserved.len(),
        candidates = report.candidates.len(),
        deleted = report.deleted.len(),
        failed = report.failed.len(),
        no_op_reason = no_op,
        "worker codebase-memory retention: preserved {}, candidates {}, deleted {}, failed {}",
        report.preserved.len(),
        report.candidates.len(),
        report.deleted.len(),
        report.failed.len(),
    );
}
