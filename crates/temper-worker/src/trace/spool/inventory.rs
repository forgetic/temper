use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt as _;
use temper_protocol_activity::AgentRunEventV1;

use super::recovery::recover_referenced_blobs;
use super::*;
use crate::trace::TraceCoordination;

/// One deterministic classification for a physical spool entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceSpoolOutcome {
    /// A live owner fence or worker-local active-run registration protects it.
    ProtectedLiveRun,
    /// A valid non-terminal stream has no live owner and can be reclaimed.
    AbandonedNonTerminalRun,
    /// A valid terminal stream is immutable but has not been compacted.
    TerminalUnacknowledgedRun,
    /// Durable acknowledgement evidence replaced the accepted payload.
    CompactedRun,
    /// The entry is malformed, unexpected, or unsafe to traverse.
    MalformedRun,
    /// Evidence already isolated outside the active spool namespace.
    QuarantinedEvidence,
}

/// Physical and logical accounting for one deterministic spool entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceSpoolEntry {
    /// Path relative to the configured spool root.
    pub path: PathBuf,
    /// Valid UTF-8 run identity when one is available.
    pub run_id: Option<String>,
    pub outcome: TraceSpoolOutcome,
    pub physical_bytes: u64,
    pub logical_reserved_bytes: u64,
}

/// Saturating counts for every spool classification.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceSpoolOutcomeCounts {
    pub protected_live_runs: u64,
    pub abandoned_non_terminal_runs: u64,
    pub terminal_unacknowledged_runs: u64,
    pub compacted_runs: u64,
    pub malformed_runs: u64,
    pub quarantined_evidence: u64,
}

/// Deterministically ordered spool inventory and aggregate quota authority.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceSpoolInventory {
    pub entries: Vec<TraceSpoolEntry>,
    pub total_physical_bytes: u64,
    pub logical_reserved_bytes: u64,
    pub dirty_run_count: u64,
    pub quarantined_physical_bytes: u64,
    pub outcomes: TraceSpoolOutcomeCounts,
}

pub(in crate::trace) const TRACE_QUARANTINE_DIR: &str = "quarantine";
pub(super) const RUN_OWNERSHIP_LOCK_FILE: &str = ".owner.lock";

/// An exclusive, non-blocking claim proving that no live [`crate::TraceRun`]
/// owns a durable run directory. Reclamation code keeps this claim alive for
/// the complete mutation boundary.
pub(super) struct TraceRunOwnershipClaim {
    file: File,
}

impl Drop for TraceRunOwnershipClaim {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

enum OwnershipInspection {
    Protected,
    Claimed(TraceRunOwnershipClaim),
    Invalid,
}

enum ValidRunState {
    NonTerminal(TraceManifestV1),
    Terminal(TraceManifestV1),
    Compacted(TraceManifestV1),
}

pub(in crate::trace) fn inventory(
    root: &Path,
    coordination: &TraceCoordination,
) -> Result<TraceSpoolInventory, TraceError> {
    let mut paths = read_dir(root)?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|source| io_error("read trace spool entry", root, source))
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();

    let mut entries = Vec::new();
    for path in paths {
        let name = path.file_name().and_then(|name| name.to_str());
        if name == Some(".spool-root.lock") && is_regular_without_symlink(&path) {
            continue;
        }
        if name == Some(TRACE_QUARANTINE_DIR) && is_directory_without_symlink(&path) {
            inventory_quarantine(root, &path, &mut entries);
            continue;
        }
        entries.push(inventory_active_entry(root, &path, coordination));
    }
    Ok(report_from_entries(entries))
}

fn inventory_quarantine(root: &Path, quarantine: &Path, entries: &mut Vec<TraceSpoolEntry>) {
    let Ok(read) = fs::read_dir(quarantine) else {
        entries.push(TraceSpoolEntry {
            path: relative_to(root, quarantine),
            run_id: None,
            outcome: TraceSpoolOutcome::QuarantinedEvidence,
            physical_bytes: u64::MAX,
            logical_reserved_bytes: 0,
        });
        return;
    };
    let evidence = read
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>();
    let Ok(mut evidence) = evidence else {
        entries.push(TraceSpoolEntry {
            path: relative_to(root, quarantine),
            run_id: None,
            outcome: TraceSpoolOutcome::QuarantinedEvidence,
            physical_bytes: u64::MAX,
            logical_reserved_bytes: 0,
        });
        return;
    };
    evidence.sort();
    entries.extend(evidence.into_iter().map(|path| TraceSpoolEntry {
        path: relative_to(root, &path),
        run_id: None,
        outcome: TraceSpoolOutcome::QuarantinedEvidence,
        physical_bytes: physical_bytes(&path),
        logical_reserved_bytes: 0,
    }));
}

fn inventory_active_entry(
    root: &Path,
    path: &Path,
    coordination: &TraceCoordination,
) -> TraceSpoolEntry {
    let relative = relative_to(root, path);
    let mut physical = physical_bytes(path);
    if !is_directory_without_symlink(path) {
        return TraceSpoolEntry {
            path: relative,
            run_id: None,
            outcome: TraceSpoolOutcome::MalformedRun,
            physical_bytes: physical,
            logical_reserved_bytes: physical,
        };
    }

    let run_id = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string);
    let reservation = manifest_reservation(path);
    let ownership = inspect_ownership(path, run_id.as_deref(), coordination);
    // A legacy abandoned spool gains a zero-byte ownership lock on first
    // inspection. Measure after claiming so the report describes the complete
    // durable directory even on filesystems that assign lock files a size.
    physical = physical_bytes(path);
    if matches!(ownership, OwnershipInspection::Protected) {
        return TraceSpoolEntry {
            path: relative,
            run_id,
            outcome: TraceSpoolOutcome::ProtectedLiveRun,
            physical_bytes: physical,
            logical_reserved_bytes: growable_reservation(reservation, physical),
        };
    }
    if matches!(ownership, OwnershipInspection::Invalid) {
        return TraceSpoolEntry {
            path: relative,
            run_id,
            outcome: TraceSpoolOutcome::MalformedRun,
            physical_bytes: physical,
            logical_reserved_bytes: growable_reservation(reservation, physical),
        };
    }

    // Keep the claim in scope until all immutable evidence has been inspected.
    let OwnershipInspection::Claimed(_claim) = ownership else {
        unreachable!("ownership inspection handled protected and invalid states")
    };
    match inspect_valid_run(path) {
        Ok(ValidRunState::NonTerminal(manifest)) => TraceSpoolEntry {
            path: relative,
            run_id: Some(manifest.run_id),
            outcome: TraceSpoolOutcome::AbandonedNonTerminalRun,
            physical_bytes: physical,
            logical_reserved_bytes: growable_reservation(
                Some(manifest.policy.max_run_bytes),
                physical,
            ),
        },
        Ok(ValidRunState::Terminal(manifest)) => TraceSpoolEntry {
            path: relative,
            run_id: Some(manifest.run_id),
            outcome: TraceSpoolOutcome::TerminalUnacknowledgedRun,
            physical_bytes: physical,
            logical_reserved_bytes: physical,
        },
        Ok(ValidRunState::Compacted(manifest)) => TraceSpoolEntry {
            path: relative,
            run_id: Some(manifest.run_id),
            outcome: TraceSpoolOutcome::CompactedRun,
            physical_bytes: physical,
            logical_reserved_bytes: physical,
        },
        Err(()) => TraceSpoolEntry {
            path: relative,
            run_id,
            outcome: TraceSpoolOutcome::MalformedRun,
            physical_bytes: physical,
            logical_reserved_bytes: growable_reservation(reservation, physical),
        },
    }
}

fn inspect_ownership(
    run_dir: &Path,
    run_id: Option<&str>,
    coordination: &TraceCoordination,
) -> OwnershipInspection {
    if run_id.is_some_and(|run_id| coordination.is_active(run_id)) {
        return OwnershipInspection::Protected;
    }
    let ownership_path = run_dir.join(RUN_OWNERSHIP_LOCK_FILE);
    match fs::symlink_metadata(&ownership_path) {
        Ok(metadata) if !metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            return OwnershipInspection::Invalid;
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return OwnershipInspection::Invalid,
    }
    let Ok((_, file)) = open_spool_owner_lock(run_dir) else {
        // Failure to prove abandonment must fence reclamation rather than
        // allowing an inaccessible run to be treated as abandoned.
        return OwnershipInspection::Protected;
    };
    match file.try_lock_exclusive() {
        Ok(()) => OwnershipInspection::Claimed(TraceRunOwnershipClaim { file }),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => OwnershipInspection::Protected,
        Err(_) => OwnershipInspection::Protected,
    }
}

fn inspect_valid_run(run_dir: &Path) -> Result<ValidRunState, ()> {
    validate_run_file_types(run_dir)?;
    let manifest: TraceManifestV1 = read_regular_json(&run_dir.join("manifest.json"))?;
    validate_manifest(&manifest).map_err(|_| ())?;
    if run_dir.file_name().and_then(|name| name.to_str()) != Some(manifest.run_id.as_str()) {
        return Err(());
    }
    let cursor: TraceAckCursorV1 = read_regular_json(&run_dir.join("acknowledgement.json"))?;
    if cursor.version != ACTIVITY_PROTOCOL_VERSION || cursor.run_id != manifest.run_id {
        return Err(());
    }

    let compacted_path = run_dir.join("compacted.json");
    if compacted_path.exists() {
        let compacted: TraceCompactedAckV1 = read_regular_json(&compacted_path)?;
        if compacted.version != ACTIVITY_PROTOCOL_VERSION
            || compacted.run_id != manifest.run_id
            || compacted.highest_contiguous_seq == 0
            || compacted.highest_contiguous_seq != cursor.highest_contiguous_seq
            || !compacted.terminal
        {
            return Err(());
        }
        return Ok(ValidRunState::Compacted(manifest));
    }

    let bytes = read_regular_bytes(&run_dir.join("events.jsonl"))?;
    let complete_len = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let mut events = Vec::<AgentRunEventV1>::new();
    for record in bytes[..complete_len].split(|byte| *byte == b'\n') {
        if !record.is_empty() {
            events.push(serde_json::from_slice(record).map_err(|_| ())?);
        }
    }
    if events.is_empty()
        || !matches!(events[0].event, AgentActivityEventV1::RunStarted(_))
        || temper_protocol_activity::validate_run_stream(&events).is_err()
    {
        return Err(());
    }
    let mut terminal_seen = false;
    for event in &events {
        if event.run_id != manifest.run_id
            || event.assignment != manifest.assignment
            || event.agent_session_id != manifest.agent_session_id
            || terminal_seen
        {
            return Err(());
        }
        terminal_seen = event.event.is_terminal();
    }
    let last_seq = events.last().map_or(0, |event| event.seq);
    if cursor.highest_contiguous_seq > last_seq {
        return Err(());
    }
    recover_referenced_blobs(run_dir, &events).map_err(|_| ())?;
    if terminal_seen {
        Ok(ValidRunState::Terminal(manifest))
    } else {
        Ok(ValidRunState::NonTerminal(manifest))
    }
}

fn validate_run_file_types(run_dir: &Path) -> Result<(), ()> {
    let mut entries = fs::read_dir(run_dir)
        .map_err(|_| ())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
    entries.sort_by_key(|entry| entry.path());
    let mut blobs_seen = false;
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(());
        };
        let metadata = fs::symlink_metadata(&path).map_err(|_| ())?;
        if name == "blobs" {
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(());
            }
            blobs_seen = true;
            for blob in fs::read_dir(&path).map_err(|_| ())? {
                let blob = blob.map_err(|_| ())?;
                let metadata = fs::symlink_metadata(blob.path()).map_err(|_| ())?;
                if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                    return Err(());
                }
            }
            continue;
        }
        if !matches!(
            name,
            "manifest.json"
                | "events.jsonl"
                | "acknowledgement.json"
                | "compacted.json"
                | ".spool.lock"
                | RUN_OWNERSHIP_LOCK_FILE
                | FORWARDING_INDEX_FILE
        ) || !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
        {
            return Err(());
        }
    }
    blobs_seen.then_some(()).ok_or(())
}

fn manifest_reservation(run_dir: &Path) -> Option<u64> {
    let manifest: TraceManifestV1 = read_regular_json(&run_dir.join("manifest.json")).ok()?;
    validate_manifest(&manifest).ok()?;
    (run_dir.file_name().and_then(|name| name.to_str()) == Some(manifest.run_id.as_str()))
        .then_some(manifest.policy.max_run_bytes)
}

fn read_regular_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, ()> {
    serde_json::from_slice(&read_regular_bytes(path)?).map_err(|_| ())
}

fn read_regular_bytes(path: &Path) -> Result<Vec<u8>, ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(());
    }
    fs::read(path).map_err(|_| ())
}

fn is_directory_without_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_dir() && !metadata.file_type().is_symlink())
}

fn is_regular_without_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
}

fn physical_bytes(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return u64::MAX;
    };
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return metadata.len();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return u64::MAX;
    };
    entries.fold(0u64, |total, entry| {
        let bytes = entry.map_or(u64::MAX, |entry| physical_bytes(&entry.path()));
        total.saturating_add(bytes)
    })
}

fn growable_reservation(reservation: Option<u64>, physical: u64) -> u64 {
    reservation.unwrap_or(physical).max(physical)
}

fn relative_to(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn report_from_entries(mut entries: Vec<TraceSpoolEntry>) -> TraceSpoolInventory {
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let mut report = TraceSpoolInventory {
        entries,
        ..TraceSpoolInventory::default()
    };
    for entry in &report.entries {
        report.total_physical_bytes = report
            .total_physical_bytes
            .saturating_add(entry.physical_bytes);
        report.logical_reserved_bytes = report
            .logical_reserved_bytes
            .saturating_add(entry.logical_reserved_bytes);
        match entry.outcome {
            TraceSpoolOutcome::ProtectedLiveRun => {
                report.outcomes.protected_live_runs =
                    report.outcomes.protected_live_runs.saturating_add(1);
                report.dirty_run_count = report.dirty_run_count.saturating_add(1);
            }
            TraceSpoolOutcome::AbandonedNonTerminalRun => {
                report.outcomes.abandoned_non_terminal_runs = report
                    .outcomes
                    .abandoned_non_terminal_runs
                    .saturating_add(1);
                report.dirty_run_count = report.dirty_run_count.saturating_add(1);
            }
            TraceSpoolOutcome::TerminalUnacknowledgedRun => {
                report.outcomes.terminal_unacknowledged_runs = report
                    .outcomes
                    .terminal_unacknowledged_runs
                    .saturating_add(1);
            }
            TraceSpoolOutcome::CompactedRun => {
                report.outcomes.compacted_runs = report.outcomes.compacted_runs.saturating_add(1);
            }
            TraceSpoolOutcome::MalformedRun => {
                report.outcomes.malformed_runs = report.outcomes.malformed_runs.saturating_add(1);
            }
            TraceSpoolOutcome::QuarantinedEvidence => {
                report.outcomes.quarantined_evidence =
                    report.outcomes.quarantined_evidence.saturating_add(1);
                report.quarantined_physical_bytes = report
                    .quarantined_physical_bytes
                    .saturating_add(entry.physical_bytes);
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_totals_saturate_instead_of_wrapping() {
        let entries = vec![
            TraceSpoolEntry {
                path: PathBuf::from("a"),
                run_id: None,
                outcome: TraceSpoolOutcome::MalformedRun,
                physical_bytes: u64::MAX,
                logical_reserved_bytes: u64::MAX,
            },
            TraceSpoolEntry {
                path: PathBuf::from("b"),
                run_id: None,
                outcome: TraceSpoolOutcome::QuarantinedEvidence,
                physical_bytes: 1,
                logical_reserved_bytes: 0,
            },
        ];
        let report = report_from_entries(entries);
        assert_eq!(report.total_physical_bytes, u64::MAX);
        assert_eq!(report.logical_reserved_bytes, u64::MAX);
        assert_eq!(report.quarantined_physical_bytes, 1);
        assert_eq!(report.outcomes.malformed_runs, 1);
        assert_eq!(report.outcomes.quarantined_evidence, 1);
    }
}
