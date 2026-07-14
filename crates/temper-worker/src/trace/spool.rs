use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityEventV1, AgentRunEventV1, BlobAttachmentV1,
    BlobReferenceV1, RunStartedV1,
};

use super::{RecoveredTraceRun, TraceError, TraceManifestV1, event_blob_references};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TraceAckCursorV1 {
    version: u32,
    run_id: String,
    highest_contiguous_seq: u64,
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

pub(super) fn recover_run(run_dir: &Path) -> Result<RecoveredTraceRun, TraceError> {
    set_private_dir(run_dir)?;
    let manifest_path = run_dir.join("manifest.json");
    let manifest: TraceManifestV1 = read_json(&manifest_path)?;
    validate_manifest(&manifest)?;
    if run_dir.file_name().and_then(|name| name.to_str()) != Some(manifest.run_id.as_str()) {
        return Err(TraceError::InvalidSpool(format!(
            "run directory does not match manifest run ID {}",
            manifest.run_id
        )));
    }

    let events_path = run_dir.join("events.jsonl");
    let events = recover_event_records(&events_path)?;
    if events.is_empty() || !matches!(events[0].event, AgentActivityEventV1::RunStarted(_)) {
        return Err(TraceError::InvalidSpool(format!(
            "run {} does not begin with run.started",
            manifest.run_id
        )));
    }
    temper_protocol_activity::validate_run_stream(&events)?;
    let mut terminal_seen = false;
    for event in &events {
        if event.run_id != manifest.run_id
            || event.assignment != manifest.assignment
            || event.agent_session_id != manifest.agent_session_id
        {
            return Err(TraceError::InvalidSpool(format!(
                "run {} event identity differs from immutable manifest",
                manifest.run_id
            )));
        }
        if terminal_seen {
            return Err(TraceError::InvalidSpool(format!(
                "run {} contains an event after its terminal event",
                manifest.run_id
            )));
        }
        terminal_seen = event.event.is_terminal();
    }

    let cursor_path = run_dir.join("acknowledgement.json");
    let cursor: TraceAckCursorV1 = read_json(&cursor_path)?;
    if cursor.version != ACTIVITY_PROTOCOL_VERSION || cursor.run_id != manifest.run_id {
        return Err(TraceError::InvalidSpool(format!(
            "run {} has a mismatched acknowledgement cursor",
            manifest.run_id
        )));
    }
    let last_seq = events.last().map_or(0, |event| event.seq);
    if cursor.highest_contiguous_seq > last_seq {
        return Err(TraceError::InvalidAcknowledgement {
            acknowledged: cursor.highest_contiguous_seq,
            last_seq,
        });
    }

    let mut references = BTreeMap::<String, BlobReferenceV1>::new();
    for event in &events {
        for reference in event_blob_references(&event.event) {
            if references
                .insert(reference.digest.clone(), reference.clone())
                .is_some_and(|existing| existing != *reference)
            {
                return Err(TraceError::InvalidSpool(
                    "one recovered blob digest has conflicting metadata".to_string(),
                ));
            }
        }
    }
    let blobs_dir = run_dir.join("blobs");
    set_private_dir(&blobs_dir)?;
    let mut blobs = Vec::with_capacity(references.len());
    for reference in references.values() {
        let path = blob_path(&blobs_dir, reference)?;
        let bytes = read_bytes(&path)?;
        let attachment = BlobAttachmentV1::from_bytes(reference.media_type, &bytes);
        if attachment.blob != *reference {
            return Err(TraceError::InvalidSpool(format!(
                "recovered blob {} does not match its event reference",
                reference.digest
            )));
        }
        blobs.push(attachment);
    }

    Ok(RecoveredTraceRun {
        manifest,
        events,
        blobs,
        acknowledged_seq: cursor.highest_contiguous_seq,
    })
}

fn recover_event_records(path: &Path) -> Result<Vec<AgentRunEventV1>, TraceError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|source| io_error("open activity records", path, source))?;
    set_private_file(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| io_error("read activity records", path, source))?;
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        let complete_len = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        file.set_len(u64::try_from(complete_len).unwrap_or(0))
            .and_then(|()| file.sync_data())
            .map_err(|source| io_error("truncate incomplete activity record", path, source))?;
        bytes.truncate(complete_len);
    }
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice)
        .collect::<Result<Vec<_>, _>>()
        .map_err(TraceError::Json)
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
    set_private_file(path)?;
    fs::read(path).map_err(|source| io_error("read trace spool file", path, source))
}

pub(super) fn read_dir(path: &Path) -> Result<fs::ReadDir, TraceError> {
    fs::read_dir(path).map_err(|source| io_error("read trace spool directory", path, source))
}

pub(super) fn create_private_dir_all(path: &Path) -> Result<(), TraceError> {
    fs::create_dir_all(path)
        .map_err(|source| io_error("create trace spool directory", path, source))?;
    set_private_dir(path)
}

pub(super) fn create_private_dir(path: &Path) -> Result<(), TraceError> {
    fs::create_dir(path).map_err(|source| io_error("create trace run directory", path, source))?;
    set_private_dir(path)
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
    let file = options
        .open(path)
        .map_err(|source| io_error("create trace spool file", path, source))?;
    set_private_file(path)?;
    Ok(file)
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
            .and_then(|()| file.sync_all())
            .map_err(|source| io_error("write atomic trace metadata", &temp_path, source))?;
        set_private_file(&temp_path)?;
        if !replace && path.exists() {
            return Err(TraceError::InvalidSpool(format!(
                "immutable trace metadata already exists at {}",
                path.display()
            )));
        }
        fs::rename(&temp_path, path)
            .map_err(|source| io_error("install atomic trace metadata", path, source))?;
        set_private_file(path)?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| io_error("sync trace metadata directory", parent, source))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

#[cfg(unix)]
pub(super) fn set_private_dir(path: &Path) -> Result<(), TraceError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|source| io_error("set private trace directory permissions", path, source))
}

#[cfg(not(unix))]
pub(super) fn set_private_dir(_path: &Path) -> Result<(), TraceError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<(), TraceError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|source| io_error("set private trace file permissions", path, source))
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<(), TraceError> {
    Ok(())
}

pub(super) fn io_error(operation: &'static str, path: &Path, source: io::Error) -> TraceError {
    TraceError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}
