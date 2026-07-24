use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityEventV1, AgentRunEventV1, BlobAttachmentV1,
    BlobReferenceV1, RunStartedV1,
};

use super::{
    RecoveredTraceRun, TraceCoordination, TraceError, TraceManifestV1, event_blob_references,
};

mod filesystem;
mod forwarding_index;
mod inventory;
mod operation_counts;
mod reclamation;
mod recovery;
pub(super) use filesystem::{repair_private_dir, repair_spool_root_permissions, sync_file_data};
use filesystem::{repair_run_permissions, sync_directory, sync_file_all};
use forwarding_index::persist_forwarding_index_best_effort;
pub(super) use forwarding_index::{acknowledge_forwarded_run, persist_forwarding_index};
pub(super) use inventory::TRACE_QUARANTINE_DIR;
pub(super) use inventory::inventory as spool_inventory;
pub use inventory::{
    TraceSpoolEntry, TraceSpoolInventory, TraceSpoolOutcome, TraceSpoolOutcomeCounts,
};
use operation_counts::*;
#[cfg(test)]
pub(super) use operation_counts::{
    TraceSpoolOperationCounts, reset_spool_operation_counts, spool_operation_counts,
};
pub use reclamation::TraceReclamationReport;
#[cfg(test)]
pub(super) use reclamation::{
    ReclamationFault, TERMINALIZATION_MARKER_FILE, set_reclamation_fault,
};
use recovery::{recover_compacted_marker, recover_run_with_offsets_locked, recover_spool_metadata};
pub(super) use recovery::{recover_forwarding_run, recover_run};

const FORWARDING_INDEX_VERSION: u32 = 1;
pub(super) const FORWARDING_INDEX_FILE: &str = ".forwarding-index.json";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TraceAckCursorV1 {
    version: u32,
    run_id: String,
    highest_contiguous_seq: u64,
}

/// Durable evidence that every record in a terminal run was accepted by the
/// engine before the worker reclaimed its payload.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceCompactedAckV1 {
    version: u32,
    run_id: String,
    highest_contiguous_seq: u64,
    terminal: bool,
}

/// Discardable forwarding metadata. The acknowledgement cursor remains the
/// sole authority; this sidecar only identifies the byte boundary from which a
/// forwarding-only recovery may safely resume validation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceForwardingIndexV1 {
    version: u32,
    run_id: String,
    highest_contiguous_seq: u64,
    event_end_offset: u64,
}

#[derive(Clone, Debug)]
pub(super) struct RecoveredForwardingRun {
    pub(super) manifest: TraceManifestV1,
    pub(super) events: Vec<AgentRunEventV1>,
    pub(super) event_end_offsets: Vec<u64>,
    pub(super) blobs: Vec<BlobAttachmentV1>,
    pub(super) acknowledged_seq: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ForwardingAcknowledgementBoundary {
    pub(super) sequence: u64,
    pub(super) event_end_offset: u64,
    pub(super) terminal: bool,
}

struct RecoveredSpoolMetadata {
    manifest: TraceManifestV1,
    cursor: TraceAckCursorV1,
}

impl TraceAckCursorV1 {
    pub(super) fn new(run_id: &str, highest_contiguous_seq: u64) -> Self {
        Self {
            version: ACTIVITY_PROTOCOL_VERSION,
            run_id: run_id.to_string(),
            highest_contiguous_seq,
        }
    }
}

fn write_acknowledgement_cursor(
    run_dir: &Path,
    run_id: &str,
    highest_contiguous_seq: u64,
) -> Result<(), TraceError> {
    let cursor = TraceAckCursorV1::new(run_id, highest_contiguous_seq);
    let bytes = serde_json::to_vec_pretty(&cursor)?;
    atomic_write(&run_dir.join("acknowledgement.json"), &bytes, true)
}

fn compact_fully_acknowledged_run(
    run_dir: &Path,
    run_id: &str,
    highest_contiguous_seq: u64,
) -> Result<(), TraceError> {
    let compacted = TraceCompactedAckV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: run_id.to_string(),
        highest_contiguous_seq,
        terminal: true,
    };
    let bytes = serde_json::to_vec_pretty(&compacted)?;
    // The marker is installed and synced before any accepted payload is
    // removed. Recovery can therefore finish an interrupted reclaim, while a
    // crash before this write leaves the complete spool.
    atomic_write(&run_dir.join("compacted.json"), &bytes, false)?;
    reclaim_compacted_payload(run_dir)
}

pub(super) fn acknowledge_recovered_run(
    run_dir: &Path,
    highest_contiguous_seq: u64,
) -> Result<bool, TraceError> {
    acknowledge_run_locked_at_root(run_dir, |run_dir| {
        let recovered = recover_run_with_offsets_locked(run_dir)?;
        if recovered.events.is_empty() {
            return if highest_contiguous_seq <= recovered.acknowledged_seq {
                Ok(false)
            } else {
                Err(TraceError::InvalidAcknowledgement {
                    acknowledged: highest_contiguous_seq,
                    last_seq: recovered.acknowledged_seq,
                })
            };
        }
        let last_seq = recovered.events.last().map_or(0, |event| event.seq);
        if highest_contiguous_seq > last_seq {
            return Err(TraceError::InvalidAcknowledgement {
                acknowledged: highest_contiguous_seq,
                last_seq,
            });
        }
        let advanced = highest_contiguous_seq > recovered.acknowledged_seq;
        let acknowledged_seq = recovered.acknowledged_seq.max(highest_contiguous_seq);
        if advanced {
            // The cursor is authoritative and must reach stable storage before
            // the discardable byte-boundary index is replaced.
            write_acknowledgement_cursor(run_dir, &recovered.manifest.run_id, acknowledged_seq)?;
        }
        let acknowledged_index = usize::try_from(acknowledged_seq.saturating_sub(1)).ok();
        let terminal = acknowledged_index
            .and_then(|index| recovered.events.get(index))
            .is_some_and(|event| event.seq == last_seq && event.event.is_terminal());
        if terminal {
            compact_fully_acknowledged_run(run_dir, &recovered.manifest.run_id, acknowledged_seq)?;
        } else {
            let event_end_offset = if acknowledged_seq == 0 {
                0
            } else {
                acknowledged_index
                    .and_then(|index| recovered.event_end_offsets.get(index))
                    .copied()
                    .ok_or_else(|| {
                        TraceError::InvalidSpool(format!(
                            "run {} has no event boundary for acknowledgement {}",
                            recovered.manifest.run_id, acknowledged_seq
                        ))
                    })?
            };
            persist_forwarding_index_best_effort(
                run_dir,
                &recovered.manifest.run_id,
                acknowledged_seq,
                event_end_offset,
            );
        }
        Ok(advanced)
    })
}

fn acknowledge_run_locked_at_root<T>(
    run_dir: &Path,
    operation: impl FnOnce(&Path) -> Result<T, TraceError>,
) -> Result<T, TraceError> {
    let root = run_dir.parent().ok_or_else(|| {
        TraceError::InvalidSpool(format!("{} has no spool root", run_dir.display()))
    })?;
    let (root_lock_path, root_lock_file) = open_spool_root_lock(root)?;
    root_lock_file.lock_exclusive().map_err(|source| {
        io_error(
            "lock aggregate trace spool for acknowledgement",
            &root_lock_path,
            source,
        )
    })?;
    let (lock_path, lock_file) = open_spool_lock(run_dir)?;
    lock_file
        .lock_exclusive()
        .map_err(|source| io_error("lock trace spool for acknowledgement", &lock_path, source))?;
    let result = operation(run_dir);
    let unlocked = fs2::FileExt::unlock(&lock_file).map_err(|source| {
        io_error(
            "unlock trace spool after acknowledgement",
            &lock_path,
            source,
        )
    });
    let root_unlocked = fs2::FileExt::unlock(&root_lock_file).map_err(|source| {
        io_error(
            "unlock aggregate trace spool after acknowledgement",
            &root_lock_path,
            source,
        )
    });
    match (result, unlocked, root_unlocked) {
        (Err(error), _, _) | (Ok(_), Err(error), _) | (Ok(_), Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(()), Ok(())) => Ok(value),
    }
}

/// Reads only the authoritative durable cursor for one known run.
///
/// This deliberately avoids recovering `events.jsonl`: terminal flush waiters
/// need cursor durability, not a full validation and payload scan.
pub(super) fn read_acknowledged_sequence(
    run_dir: &Path,
    expected_run_id: &str,
) -> Result<u64, TraceError> {
    let (lock_path, lock_file) = open_spool_lock(run_dir)?;
    lock_file.lock_exclusive().map_err(|source| {
        io_error(
            "lock trace spool for acknowledgement read",
            &lock_path,
            source,
        )
    })?;
    let cursor_path = run_dir.join("acknowledgement.json");
    let cursor = read_json::<TraceAckCursorV1>(&cursor_path);
    let unlocked = fs2::FileExt::unlock(&lock_file).map_err(|source| {
        io_error(
            "unlock trace spool after acknowledgement read",
            &lock_path,
            source,
        )
    });
    match (cursor, unlocked) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(cursor), Ok(()))
            if cursor.version == ACTIVITY_PROTOCOL_VERSION && cursor.run_id == expected_run_id =>
        {
            Ok(cursor.highest_contiguous_seq)
        }
        (Ok(_), Ok(())) => Err(TraceError::InvalidSpool(format!(
            "run {expected_run_id} has a mismatched acknowledgement cursor"
        ))),
    }
}

pub(super) fn open_spool_root_lock(root: &Path) -> Result<(PathBuf, File), TraceError> {
    open_named_lock(root, ".spool-root.lock")
}

pub(super) fn open_spool_lock(run_dir: &Path) -> Result<(PathBuf, File), TraceError> {
    open_named_lock(run_dir, ".spool.lock")
}

pub(super) fn open_spool_owner_lock(run_dir: &Path) -> Result<(PathBuf, File), TraceError> {
    open_named_lock(run_dir, inventory::RUN_OWNERSHIP_LOCK_FILE)
}

fn open_named_lock(directory: &Path, name: &str) -> Result<(PathBuf, File), TraceError> {
    let path = directory.join(name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            return Err(TraceError::InvalidSpool(format!(
                "trace spool lock is not a regular file: {}",
                path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_error("inspect trace spool lock", &path, source)),
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .map_err(|source| io_error("open trace spool lock", &path, source))?;
    Ok((path, file))
}

pub(super) fn ensure_aggregate_spool_capacity(
    root: &Path,
    coordination: &TraceCoordination,
    requested: u64,
    limit: u64,
) -> Result<(), TraceError> {
    let report = inventory::inventory(root, coordination)?;
    if report
        .logical_reserved_bytes
        .checked_add(requested)
        .is_none_or(|projected| projected > limit)
    {
        Err(TraceError::AggregateQuotaExceeded { limit })
    } else {
        Ok(())
    }
}

fn reclaim_compacted_payload(run_dir: &Path) -> Result<(), TraceError> {
    let events_path = run_dir.join("events.jsonl");
    let events_metadata = fs::symlink_metadata(&events_path).map_err(|source| {
        io_error(
            "inspect acknowledged activity payload",
            &events_path,
            source,
        )
    })?;
    if !events_metadata.is_file() || events_metadata.file_type().is_symlink() {
        return Err(TraceError::InvalidSpool(format!(
            "acknowledged event payload is not a regular file: {}",
            events_path.display()
        )));
    }

    // Validate the complete blob directory before changing either payload.
    // A corrupt sibling must not leave reclamation half-complete.
    let blobs_dir = run_dir.join("blobs");
    let mut blob_payloads = Vec::new();
    for entry in read_dir(&blobs_dir)? {
        let entry = entry
            .map_err(|source| io_error("read acknowledged activity blobs", &blobs_dir, source))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|source| io_error("inspect acknowledged activity blob", &path, source))?;
        if metadata.is_file() && !metadata.file_type().is_symlink() {
            blob_payloads.push(path);
        } else {
            return Err(TraceError::InvalidSpool(format!(
                "acknowledged blob payload is not a regular file: {}",
                path.display()
            )));
        }
    }

    if events_metadata.len() == 0 && blob_payloads.is_empty() {
        return Ok(());
    }

    if events_metadata.len() > 0 {
        let events = OpenOptions::new()
            .write(true)
            .open(&events_path)
            .map_err(|source| {
                io_error("open acknowledged activity payload", &events_path, source)
            })?;
        events.set_len(0).map_err(|source| {
            io_error(
                "truncate acknowledged activity payload",
                &events_path,
                source,
            )
        })?;
        record_truncation();
        sync_file_all(&events).map_err(|source| {
            io_error(
                "sync truncated acknowledged activity payload",
                &events_path,
                source,
            )
        })?;
    }

    if !blob_payloads.is_empty() {
        for path in blob_payloads {
            fs::remove_file(&path)
                .map_err(|source| io_error("remove acknowledged activity blob", &path, source))?;
            record_deletion();
        }
        sync_directory(&blobs_dir)
            .map_err(|source| io_error("sync compacted activity blobs", &blobs_dir, source))?;
    }
    Ok(())
}

fn recover_event_records(
    path: &Path,
    start_offset: u64,
) -> Result<(Vec<AgentRunEventV1>, Vec<u64>), TraceError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|source| io_error("open activity records", path, source))?;
    file.seek(SeekFrom::Start(start_offset))
        .map_err(|source| io_error("seek activity records", path, source))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| io_error("read activity records", path, source))?;
    record_event_payload_bytes_read(bytes.len());
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        let complete_len = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let complete_len_u64 = u64::try_from(complete_len).unwrap_or(u64::MAX);
        let truncate_to = start_offset.checked_add(complete_len_u64).ok_or_else(|| {
            TraceError::InvalidSpool("activity record byte offset overflowed".to_string())
        })?;
        file.set_len(truncate_to)
            .map_err(|source| io_error("truncate incomplete activity record", path, source))?;
        record_truncation();
        sync_file_data(&file)
            .map_err(|source| io_error("sync truncated activity record", path, source))?;
        bytes.truncate(complete_len);
    }

    let mut events = Vec::new();
    let mut event_end_offsets = Vec::new();
    let mut consumed = 0u64;
    for record in bytes.split_inclusive(|byte| *byte == b'\n') {
        consumed = consumed
            .checked_add(u64::try_from(record.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| {
                TraceError::InvalidSpool("activity record byte offset overflowed".to_string())
            })?;
        let line = record.strip_suffix(b"\n").unwrap_or(record);
        if line.is_empty() {
            continue;
        }
        events.push(serde_json::from_slice(line)?);
        event_end_offsets.push(start_offset.checked_add(consumed).ok_or_else(|| {
            TraceError::InvalidSpool("activity record byte offset overflowed".to_string())
        })?);
    }
    Ok((events, event_end_offsets))
}

#[cfg(test)]
pub(super) fn reset_event_payload_bytes_read() {
    reset_spool_operation_counts();
}

#[cfg(test)]
pub(super) fn event_payload_bytes_read() -> u64 {
    spool_operation_counts().event_payload_bytes_read
}

pub(super) fn validate_manifest(manifest: &TraceManifestV1) -> Result<(), TraceError> {
    manifest.policy.validate()?;
    let started = AgentRunEventV1 {
        version: manifest.version,
        run_id: manifest.run_id.clone(),
        seq: 1,
        occurred_at: manifest.started_at.clone(),
        elapsed_ms: 0,
        assignment: manifest.assignment.clone(),
        agent_session_id: manifest.agent_session_id.clone(),
        scope: manifest.main_scope.clone(),
        turn: None,
        event: AgentActivityEventV1::RunStarted(RunStartedV1 {
            capture: manifest.policy.capture,
        }),
    };
    started.validate()?;
    Ok(())
}

pub(super) fn blob_path(
    blobs_dir: &Path,
    reference: &BlobReferenceV1,
) -> Result<PathBuf, TraceError> {
    reference.validate()?;
    let digest = reference
        .digest
        .strip_prefix("sha256:")
        .ok_or_else(|| TraceError::InvalidSpool("blob digest does not use sha256".to_string()))?;
    Ok(blobs_dir.join(digest))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, TraceError> {
    serde_json::from_slice(&read_bytes(path)?).map_err(TraceError::Json)
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, TraceError> {
    fs::read(path).map_err(|source| io_error("read trace spool file", path, source))
}

fn read_blob_bytes(path: &Path) -> Result<Vec<u8>, TraceError> {
    let bytes =
        fs::read(path).map_err(|source| io_error("read trace blob payload", path, source))?;
    record_blob_payload_bytes_read(bytes.len());
    Ok(bytes)
}

pub(super) fn read_dir(path: &Path) -> Result<fs::ReadDir, TraceError> {
    fs::read_dir(path).map_err(|source| io_error("read trace spool directory", path, source))
}

pub(super) fn create_private_dir_all(path: &Path) -> Result<(), TraceError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|source| io_error("create trace spool directory", path, source))?;
    // The configured root may predate trace capture (or be supplied by a
    // caller as an existing directory). Run creation is a deliberate boundary
    // where that legacy root can be repaired once, unlike recurring reads.
    repair_private_dir(path)
}

pub(super) fn create_private_dir(path: &Path) -> Result<(), TraceError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|source| io_error("create trace run directory", path, source))
}

pub(super) fn create_private_file(path: &Path, create_new: bool) -> Result<File, TraceError> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .append(true)
        .create(!create_new)
        .create_new(create_new);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|source| io_error("create trace spool file", path, source))
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8], replace: bool) -> Result<(), TraceError> {
    let parent = path.parent().ok_or_else(|| {
        TraceError::InvalidSpool(format!("{} has no parent directory", path.display()))
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            TraceError::InvalidSpool("trace metadata filename is not UTF-8".to_string())
        })?;
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let write_result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp_path)
            .map_err(|source| io_error("create atomic trace metadata", &temp_path, source))?;
        file.write_all(bytes)
            .and_then(|()| sync_file_all(&file))
            .map_err(|source| io_error("write atomic trace metadata", &temp_path, source))?;
        if !replace && path.exists() {
            return Err(TraceError::InvalidSpool(format!(
                "immutable trace metadata already exists at {}",
                path.display()
            )));
        }
        fs::rename(&temp_path, path)
            .map_err(|source| io_error("install atomic trace metadata", path, source))?;
        sync_directory(parent)
            .map_err(|source| io_error("sync trace metadata directory", parent, source))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

pub(super) fn io_error(operation: &'static str, path: &Path, source: io::Error) -> TraceError {
    TraceError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}
