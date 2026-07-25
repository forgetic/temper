use std::io::Write as _;

use serde::{Deserialize, Serialize};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityEventV1, AgentRunEventV1, FailureCodeV1, FailureInfoV1,
    RunFailedV1,
};
use temper_protocol_worker::FailureClass;

use crate::trace::{TraceCollector, host_failure_summary};

use super::inventory::{
    OwnershipInspection, ValidRunState, inspect_ownership, inspect_valid_run,
    is_directory_without_symlink,
};
use super::*;

pub(in crate::trace) const TERMINALIZATION_MARKER_FILE: &str = "terminalization.json";

/// Outcome counts from one bounded reclamation pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceReclamationReport {
    /// Dirty, malformed, or protected entries inspected during this pass.
    pub examined_runs: u64,
    /// Abandoned streams that gained a durable synthetic terminal event.
    pub terminalized_runs: u64,
    /// Malformed entries atomically isolated from the active spool.
    pub quarantined_runs: u64,
    /// Entries skipped because a live owner still held their lifetime fence.
    pub protected_runs: u64,
    /// Per-entry failures isolated without stopping healthy siblings.
    pub failed_runs: u64,
    /// Dirty or malformed active entries left after the pass.
    pub remaining_dirty_runs: u64,
    /// Physical bytes retained across active and quarantined evidence.
    pub physical_used_bytes: u64,
    /// Logical bytes still charged against aggregate trace admission.
    pub logical_reserved_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceTerminalizationMarkerV1 {
    version: u32,
    run_id: String,
    terminal_event: AgentRunEventV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReclaimedEntry {
    Terminalized,
    Quarantined,
    Protected,
    Unchanged,
}

impl TraceCollector {
    /// Terminalizes abandoned valid streams and quarantines malformed entries.
    ///
    /// Work is deterministic; `max_runs` bounds actionable abandoned or
    /// malformed entries, while cheap protected-run skips do not consume the
    /// budget or starve healthy siblings. Every mutation occurs under the
    /// aggregate root lock and, for a run directory, its per-run lock.
    /// The lifetime ownership fence is claimed non-blockingly immediately
    /// before validation and mutation; a live run is skipped rather than
    /// waited on or modified.
    pub fn reclaim_abandoned_runs(
        &self,
        max_runs: usize,
    ) -> Result<TraceReclamationReport, TraceError> {
        let Some(root) = self.config.spool_root.as_deref() else {
            return Ok(TraceReclamationReport::default());
        };
        match fs::symlink_metadata(root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(TraceReclamationReport::default());
            }
            Err(source) => return Err(io_error("inspect trace spool root", root, source)),
            Ok(metadata) if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() => {
                return Err(TraceError::InvalidSpool(format!(
                    "trace spool root is not a regular directory: {}",
                    root.display()
                )));
            }
            Ok(_) => {}
        }

        repair_spool_root_permissions(root)?;
        let (root_lock_path, root_lock_file) = open_spool_root_lock(root)?;
        root_lock_file.lock_exclusive().map_err(|source| {
            io_error(
                "lock aggregate trace spool for reclamation",
                &root_lock_path,
                source,
            )
        })?;
        let result = reclaim_locked(root, &self.coordination, max_runs);
        let unlocked = fs2::FileExt::unlock(&root_lock_file).map_err(|source| {
            io_error(
                "unlock aggregate trace spool after reclamation",
                &root_lock_path,
                source,
            )
        });
        match (result, unlocked) {
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
            (Ok(report), Ok(())) => Ok(report),
        }
    }
}

fn reclaim_locked(
    root: &Path,
    coordination: &TraceCoordination,
    max_runs: usize,
) -> Result<TraceReclamationReport, TraceError> {
    let inventory = spool_inventory(root, coordination)?;
    let candidates = inventory.entries.into_iter().filter(|entry| {
        matches!(
            entry.outcome,
            TraceSpoolOutcome::ProtectedLiveRun
                | TraceSpoolOutcome::AbandonedNonTerminalRun
                | TraceSpoolOutcome::MalformedRun
        )
    });
    let mut report = TraceReclamationReport::default();
    let mut attempted = 0usize;
    for entry in candidates {
        if entry.outcome == TraceSpoolOutcome::ProtectedLiveRun {
            report.examined_runs = report.examined_runs.saturating_add(1);
            report.protected_runs = report.protected_runs.saturating_add(1);
            continue;
        }
        if attempted >= max_runs {
            continue;
        }
        attempted = attempted.saturating_add(1);
        report.examined_runs = report.examined_runs.saturating_add(1);
        match reclaim_entry(root, coordination, &entry) {
            Ok(ReclaimedEntry::Terminalized) => {
                report.terminalized_runs = report.terminalized_runs.saturating_add(1);
            }
            Ok(ReclaimedEntry::Quarantined) => {
                report.quarantined_runs = report.quarantined_runs.saturating_add(1);
            }
            Ok(ReclaimedEntry::Protected) => {
                report.protected_runs = report.protected_runs.saturating_add(1);
            }
            Ok(ReclaimedEntry::Unchanged) => {}
            Err(error) => {
                report.failed_runs = report.failed_runs.saturating_add(1);
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.reclamation_run_failed",
                    spool = %entry.path.display(),
                    %error,
                    "worker could not reclaim one activity spool and continued with siblings"
                );
            }
        }
    }
    let remaining = spool_inventory(root, coordination)?;
    report.remaining_dirty_runs = remaining
        .dirty_run_count
        .saturating_add(remaining.outcomes.malformed_runs);
    report.physical_used_bytes = remaining.total_physical_bytes;
    report.logical_reserved_bytes = remaining.logical_reserved_bytes;
    Ok(report)
}

fn reclaim_entry(
    root: &Path,
    coordination: &TraceCoordination,
    entry: &TraceSpoolEntry,
) -> Result<ReclaimedEntry, TraceError> {
    let path = root.join(&entry.path);
    if !is_directory_without_symlink(&path) {
        return quarantine_entry(root, &path).map(|()| ReclaimedEntry::Quarantined);
    }

    let (lock_path, lock_file) = match open_spool_lock(&path) {
        Ok(lock) => lock,
        Err(_lock_error) if entry.outcome == TraceSpoolOutcome::MalformedRun => {
            // Even a malformed per-run lock must not bypass a still-valid
            // lifetime owner. Claim that fence immediately before the rename;
            // only malformed ownership metadata itself is non-claimable.
            let run_id = path.file_name().and_then(|name| name.to_str());
            return match inspect_ownership(&path, run_id, coordination) {
                OwnershipInspection::Protected => Ok(ReclaimedEntry::Protected),
                OwnershipInspection::Claimed(_claim) => {
                    quarantine_entry(root, &path).map(|()| ReclaimedEntry::Quarantined)
                }
                OwnershipInspection::Invalid => {
                    quarantine_entry(root, &path).map(|()| ReclaimedEntry::Quarantined)
                }
            };
        }
        Err(error) => return Err(error),
    };
    lock_file.lock_exclusive().map_err(|source| {
        io_error(
            "lock trace spool for abandoned-run reclamation",
            &lock_path,
            source,
        )
    })?;
    let result = reclaim_run_locked(root, &path, coordination);
    let unlocked = fs2::FileExt::unlock(&lock_file).map_err(|source| {
        io_error(
            "unlock trace spool after abandoned-run reclamation",
            &lock_path,
            source,
        )
    });
    match (result, unlocked) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(outcome), Ok(())) => Ok(outcome),
    }
}

fn reclaim_run_locked(
    root: &Path,
    run_dir: &Path,
    coordination: &TraceCoordination,
) -> Result<ReclaimedEntry, TraceError> {
    let run_id = run_dir.file_name().and_then(|name| name.to_str());
    let ownership = inspect_ownership(run_dir, run_id, coordination);
    let _claim = match ownership {
        OwnershipInspection::Protected => return Ok(ReclaimedEntry::Protected),
        OwnershipInspection::Invalid => {
            quarantine_entry(root, run_dir)?;
            return Ok(ReclaimedEntry::Quarantined);
        }
        OwnershipInspection::Claimed(claim) => claim,
    };

    match inspect_valid_run(run_dir) {
        Err(()) => {
            quarantine_entry(root, run_dir)?;
            Ok(ReclaimedEntry::Quarantined)
        }
        Ok(ValidRunState::Compacted(_)) => Ok(ReclaimedEntry::Unchanged),
        Ok(ValidRunState::Terminal(_)) => {
            validate_terminalization_marker_for_terminal(run_dir)?;
            Ok(ReclaimedEntry::Unchanged)
        }
        Ok(ValidRunState::NonTerminal(_)) => terminalize_abandoned_run(run_dir, coordination),
    }
}

fn terminalize_abandoned_run(
    run_dir: &Path,
    coordination: &TraceCoordination,
) -> Result<ReclaimedEntry, TraceError> {
    // The non-mutating inventory validation has already proved the complete
    // prefix and all referenced blobs. This existing recovery primitive now
    // truncates only an incomplete final JSONL fragment before mutation.
    let recovered = recover_run_with_offsets_locked(run_dir)?;
    let Some(last) = recovered.events.last() else {
        return Err(TraceError::InvalidSpool(
            "abandoned run has no durable event".to_string(),
        ));
    };
    if last.event.is_terminal() {
        validate_terminalization_marker_for_terminal(run_dir)?;
        return Ok(ReclaimedEntry::Unchanged);
    }

    let marker = expected_terminalization_marker(&recovered.manifest, last)?;
    persist_terminalization_marker(run_dir, &marker)?;
    inject_reclamation_failure(ReclamationFault::AfterMarker)?;

    let events_path = run_dir.join("events.jsonl");
    let mut bytes = serde_json::to_vec(&marker.terminal_event)?;
    bytes.push(b'\n');
    let mut events = OpenOptions::new()
        .append(true)
        .open(&events_path)
        .map_err(|source| io_error("open abandoned activity records", &events_path, source))?;
    events
        .write_all(&bytes)
        .and_then(|()| sync_file_data(&events))
        .map_err(|source| {
            io_error(
                "append and sync abandoned-run terminal event",
                &events_path,
                source,
            )
        })?;
    inject_reclamation_failure(ReclamationFault::AfterTerminalAppend)?;
    coordination.publish_append(&recovered.manifest.run_id);
    Ok(ReclaimedEntry::Terminalized)
}

fn expected_terminalization_marker(
    manifest: &TraceManifestV1,
    last: &AgentRunEventV1,
) -> Result<TraceTerminalizationMarkerV1, TraceError> {
    let seq = last
        .seq
        .checked_add(1)
        .ok_or_else(|| TraceError::InvalidSpool("activity sequence overflowed".to_string()))?;
    let terminal_event = AgentRunEventV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: manifest.run_id.clone(),
        seq,
        // Deriving both clocks from the immutable prefix makes an interrupted
        // marker installation exactly reproducible without persisting runtime
        // or provider diagnostics.
        occurred_at: last.occurred_at.clone(),
        elapsed_ms: last.elapsed_ms,
        assignment: manifest.assignment.clone(),
        agent_session_id: manifest.agent_session_id.clone(),
        scope: manifest.main_scope.clone(),
        turn: None,
        event: AgentActivityEventV1::RunFailed(RunFailedV1 {
            failure: FailureInfoV1 {
                code: FailureCodeV1::Internal,
                message: host_failure_summary(FailureClass::Permanent).to_string(),
                retryable: false,
            },
        }),
    };
    terminal_event.validate()?;
    Ok(TraceTerminalizationMarkerV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: manifest.run_id.clone(),
        terminal_event,
    })
}

fn persist_terminalization_marker(
    run_dir: &Path,
    marker: &TraceTerminalizationMarkerV1,
) -> Result<(), TraceError> {
    let path = run_dir.join(TERMINALIZATION_MARKER_FILE);
    let bytes = serde_json::to_vec_pretty(marker)?;
    if fs::read(&path).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }

    let exists = fs::symlink_metadata(&path).is_ok();
    let mut options = OpenOptions::new();
    options.write(true);
    if exists {
        options.truncate(true);
    } else {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|source| io_error("create terminalization marker", &path, source))?;
    file.write_all(&bytes)
        .and_then(|()| sync_file_all(&file))
        .map_err(|source| io_error("write terminalization marker", &path, source))?;
    sync_directory(run_dir)
        .map_err(|source| io_error("sync terminalization marker directory", run_dir, source))
}

fn validate_terminalization_marker_for_terminal(run_dir: &Path) -> Result<(), TraceError> {
    let path = run_dir.join(TERMINALIZATION_MARKER_FILE);
    if !path.exists() {
        return Ok(());
    }
    let marker: TraceTerminalizationMarkerV1 = serde_json::from_slice(
        &fs::read(&path)
            .map_err(|source| io_error("read terminalization marker", &path, source))?,
    )?;
    let recovered = recover_run_with_offsets_locked(run_dir)?;
    let Some(last) = recovered.events.last() else {
        return Err(TraceError::InvalidSpool(
            "terminalization marker has no terminal event".to_string(),
        ));
    };
    if marker.version != ACTIVITY_PROTOCOL_VERSION
        || marker.run_id != recovered.manifest.run_id
        || marker.terminal_event != *last
    {
        return Err(TraceError::InvalidSpool(format!(
            "run {} has a mismatched terminalization marker",
            recovered.manifest.run_id
        )));
    }
    Ok(())
}

fn quarantine_entry(root: &Path, source: &Path) -> Result<(), TraceError> {
    let quarantine = root.join(TRACE_QUARANTINE_DIR);
    match fs::symlink_metadata(&quarantine) {
        Ok(metadata) if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() => {
            return Err(TraceError::InvalidSpool(format!(
                "trace quarantine is not a regular directory: {}",
                quarantine.display()
            )));
        }
        Ok(_) => repair_private_dir(&quarantine)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_private_dir(&quarantine)?;
            sync_directory(root)
                .map_err(|source| io_error("sync new trace quarantine", root, source))?;
        }
        Err(source) => {
            return Err(io_error("inspect trace quarantine", &quarantine, source));
        }
    }

    let name = source
        .file_name()
        .map_or_else(|| "evidence".into(), |name| name.to_string_lossy());
    let mut suffix = 0u64;
    let target = loop {
        let candidate = if suffix == 0 {
            quarantine.join(format!("{name}.bad"))
        } else {
            quarantine.join(format!("{name}.{suffix}.bad"))
        };
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => break candidate,
            Err(source) => {
                return Err(io_error(
                    "inspect trace quarantine target",
                    &candidate,
                    source,
                ));
            }
            Ok(_) => {}
        }
        suffix = suffix.checked_add(1).ok_or_else(|| {
            TraceError::InvalidSpool("trace quarantine name space exhausted".to_string())
        })?;
    };
    inject_reclamation_failure(ReclamationFault::BeforeQuarantineRename)?;
    fs::rename(source, &target)
        .map_err(|source| io_error("quarantine malformed trace spool", &target, source))?;
    sync_directory(&quarantine)
        .map_err(|source| io_error("sync quarantined trace evidence", &quarantine, source))?;
    sync_directory(root)
        .map_err(|source| io_error("sync active trace spool after quarantine", root, source))
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::trace) enum ReclamationFault {
    AfterMarker,
    AfterTerminalAppend,
    BeforeQuarantineRename,
}

#[cfg(test)]
thread_local! {
    static RECLAMATION_FAULT: std::cell::Cell<Option<ReclamationFault>> = const {
        std::cell::Cell::new(None)
    };
}

#[cfg(test)]
pub(in crate::trace) fn set_reclamation_fault(fault: Option<ReclamationFault>) {
    RECLAMATION_FAULT.with(|injected| injected.set(fault));
}

#[cfg(test)]
fn inject_reclamation_failure(point: ReclamationFault) -> Result<(), TraceError> {
    RECLAMATION_FAULT.with(|injected| {
        if injected.get() == Some(point) {
            injected.set(None);
            Err(TraceError::InvalidSpool(format!(
                "injected reclamation failure at {point:?}"
            )))
        } else {
            Ok(())
        }
    })
}

#[cfg(not(test))]
#[derive(Clone, Copy)]
enum ReclamationFault {
    AfterMarker,
    AfterTerminalAppend,
    BeforeQuarantineRename,
}

#[cfg(not(test))]
fn inject_reclamation_failure(_point: ReclamationFault) -> Result<(), TraceError> {
    Ok(())
}
