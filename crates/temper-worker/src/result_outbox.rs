// SPDX-License-Identifier: MPL-2.0

//! Crash-safe terminal result outbox.
//!
//! A worker records the exact protocol result before releasing local capacity.
//! Files are published with write/fsync/rename/directory-fsync and survive a
//! worker restart until the daemon returns a matching [`Release`].

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use temper_protocol_worker::{JobResult, Release, ReleaseDisposition};
use uuid::Uuid;

pub const RESULT_OUTBOX_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResultAssignmentIdentity {
    pub worker_id: String,
    pub job_id: String,
    pub attempt_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultDeliveryState {
    Pending,
}

/// Versioned durable representation of one exact terminal result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultOutboxEntry {
    pub version: u32,
    pub entry_id: String,
    pub assignment: ResultAssignmentIdentity,
    pub created_at: String,
    pub delivery: ResultDeliveryState,
    pub result: JobResult,
}

impl ResultOutboxEntry {
    pub fn from_result(result: JobResult) -> Result<Self, ResultOutboxError> {
        let attempt_id = result
            .attempt_id
            .as_deref()
            .filter(|attempt| !attempt.trim().is_empty())
            .ok_or(ResultOutboxError::MissingAttemptId)?;
        Ok(Self {
            version: RESULT_OUTBOX_VERSION,
            entry_id: Uuid::new_v4().to_string(),
            assignment: ResultAssignmentIdentity {
                worker_id: result.worker_id.clone(),
                job_id: result.job_id.clone(),
                attempt_id: attempt_id.to_string(),
            },
            created_at: Utc::now().to_rfc3339(),
            delivery: ResultDeliveryState::Pending,
            result,
        })
    }

    pub fn matches_release(&self, release: &Release) -> bool {
        release.worker_id == self.assignment.worker_id
            && release.job_id == self.assignment.job_id
            && release.attempt_id.as_deref() == Some(self.assignment.attempt_id.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResultOutboxError {
    #[error("terminal result is missing an attempt_id")]
    MissingAttemptId,
    #[error("outbox entry is invalid: {0}")]
    InvalidEntry(String),
    #[error("outbox I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("outbox serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("release acknowledgement does not match outbox entry {0}")]
    MismatchedRelease(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultAcknowledgement {
    Accepted,
    Superseded,
    Reclaimed,
}

/// Filesystem-backed result outbox rooted below the resolved private result
/// directory.
#[derive(Clone, Debug)]
pub struct ResultOutbox {
    root: PathBuf,
}

impl ResultOutbox {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn record(&self, result: JobResult) -> Result<ResultOutboxEntry, ResultOutboxError> {
        self.prepare()?;
        let entry = ResultOutboxEntry::from_result(result)?;
        let bytes = serde_json::to_vec_pretty(&entry)?;
        let pending = self.pending_dir();
        let final_path = self.entry_path(&entry.entry_id);
        let temp_path = pending.join(format!(".{}.{}.tmp", entry.entry_id, Uuid::new_v4()));

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp_path)
            .map_err(|source| io_error(&temp_path, source))?;
        if let Err(source) = (|| {
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temp_path, &final_path)?;
            sync_directory(&pending)?;
            Ok::<_, io::Error>(())
        })() {
            let _ = fs::remove_file(&temp_path);
            return Err(io_error(&final_path, source));
        }
        Ok(entry)
    }

    /// Loads every valid pending entry, removes interrupted temporary files,
    /// and atomically quarantines malformed/non-regular entries.
    pub fn load(&self) -> Result<Vec<ResultOutboxEntry>, ResultOutboxError> {
        self.prepare()?;
        let pending = self.pending_dir();
        let mut paths = fs::read_dir(&pending)
            .map_err(|source| io_error(&pending, source))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| io_error(&pending, source))?;
        paths.sort_by_key(|entry| entry.file_name());

        let mut loaded = Vec::new();
        for directory_entry in paths {
            let path = directory_entry.path();
            let name = directory_entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".tmp") {
                fs::remove_file(&path).map_err(|source| io_error(&path, source))?;
                continue;
            }
            let parsed = directory_entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_file())
                .and_then(|_| fs::read(&path).ok())
                .and_then(|bytes| serde_json::from_slice::<ResultOutboxEntry>(&bytes).ok())
                .and_then(|entry| validate_entry(entry, &name).ok());
            match parsed {
                Some(entry) => loaded.push(entry),
                None => {
                    tracing::warn!(
                        target: "temper_worker",
                        path = %path.display(),
                        "worker: quarantining malformed durable result outbox entry"
                    );
                    self.quarantine_path(&path, &name)?;
                }
            }
        }
        sync_directory(&pending).map_err(|source| io_error(&pending, source))?;
        loaded.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.entry_id.cmp(&right.entry_id))
        });
        Ok(loaded)
    }

    /// Deletes one entry only for an exact, terminal release acknowledgement.
    /// Missing files are accepted so compaction is idempotent after a crash.
    pub fn acknowledge(
        &self,
        entry: &ResultOutboxEntry,
        release: &Release,
    ) -> Result<ResultAcknowledgement, ResultOutboxError> {
        if !entry.matches_release(release) {
            return Err(ResultOutboxError::MismatchedRelease(entry.entry_id.clone()));
        }
        let acknowledgement = match release.disposition {
            ReleaseDisposition::Accepted => ResultAcknowledgement::Accepted,
            ReleaseDisposition::Superseded => ResultAcknowledgement::Superseded,
            ReleaseDisposition::Reclaimed => ResultAcknowledgement::Reclaimed,
        };
        let path = self.entry_path(&entry.entry_id);
        match fs::remove_file(&path) {
            Ok(()) => sync_directory(&self.pending_dir())
                .map_err(|source| io_error(self.pending_dir(), source))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_error(path, source)),
        }
        Ok(acknowledgement)
    }

    /// Moves a permanently rejected result out of replay while retaining the
    /// exact payload for operator inspection.
    pub fn reject(&self, entry: &ResultOutboxEntry, reason: &str) -> Result<(), ResultOutboxError> {
        self.prepare()?;
        let source = self.entry_path(&entry.entry_id);
        if !source.exists() {
            return Ok(());
        }
        let rejected = self.rejected_dir();
        let target = rejected.join(format!("{}.json", entry.entry_id));
        fs::rename(&source, &target).map_err(|source| io_error(&target, source))?;
        let reason_path = rejected.join(format!("{}.reason.txt", entry.entry_id));
        atomic_write(&reason_path, reason.as_bytes())?;
        sync_directory(&rejected).map_err(|source| io_error(&rejected, source))?;
        sync_directory(&self.pending_dir())
            .map_err(|source| io_error(self.pending_dir(), source))?;
        Ok(())
    }

    fn prepare(&self) -> Result<(), ResultOutboxError> {
        create_private_dir(&self.root)?;
        create_private_dir(&self.pending_dir())?;
        create_private_dir(&self.quarantine_dir())?;
        create_private_dir(&self.rejected_dir())?;
        Ok(())
    }

    fn pending_dir(&self) -> PathBuf {
        self.root.join("pending")
    }

    fn quarantine_dir(&self) -> PathBuf {
        self.root.join("quarantine")
    }

    fn rejected_dir(&self) -> PathBuf {
        self.root.join("rejected")
    }

    fn entry_path(&self, entry_id: &str) -> PathBuf {
        self.pending_dir().join(format!("{entry_id}.json"))
    }

    fn quarantine_path(&self, source: &Path, name: &str) -> Result<(), ResultOutboxError> {
        let quarantine = self.quarantine_dir();
        let target = quarantine.join(format!("{name}.{}.bad", Uuid::new_v4()));
        fs::rename(source, &target).map_err(|source| io_error(&target, source))?;
        sync_directory(&quarantine).map_err(|source| io_error(&quarantine, source))?;
        Ok(())
    }
}

fn validate_entry(
    entry: ResultOutboxEntry,
    file_name: &str,
) -> Result<ResultOutboxEntry, ResultOutboxError> {
    if entry.version != RESULT_OUTBOX_VERSION {
        return Err(ResultOutboxError::InvalidEntry(format!(
            "unsupported version {}",
            entry.version
        )));
    }
    if file_name != format!("{}.json", entry.entry_id)
        || entry.assignment.worker_id != entry.result.worker_id
        || entry.assignment.job_id != entry.result.job_id
        || entry.result.attempt_id.as_deref() != Some(entry.assignment.attempt_id.as_str())
        || entry.assignment.attempt_id.trim().is_empty()
    {
        return Err(ResultOutboxError::InvalidEntry(
            "file name or assignment identity does not match result".to_string(),
        ));
    }
    Ok(entry)
}

fn create_private_dir(path: &Path) -> Result<(), ResultOutboxError> {
    fs::create_dir_all(path).map_err(|source| io_error(path, source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|source| io_error(path, source))?;
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ResultOutboxError> {
    let parent = path.parent().ok_or_else(|| {
        ResultOutboxError::InvalidEntry(format!("{} has no parent", path.display()))
    })?;
    let temp = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|source| io_error(&temp, source))?;
    file.write_all(bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .and_then(|_| fs::rename(&temp, path))
        .map_err(|source| io_error(path, source))?;
    sync_directory(parent).map_err(|source| io_error(parent, source))?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn io_error(path: impl Into<PathBuf>, source: io::Error) -> ResultOutboxError {
    ResultOutboxError::Io {
        path: path.into(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use temper_protocol_worker::{
        JobResult, Release, ReleaseDisposition, ResultStatus, WORKER_PROTOCOL_VERSION,
    };

    use super::*;

    fn result(attempt: &str) -> JobResult {
        JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-a".to_string(),
            job_id: "job-a".to_string(),
            attempt_id: Some(attempt.to_string()),
            status: ResultStatus::Success,
            repos: Vec::new(),
            verdict: Some("done".to_string()),
            title: None,
            body: None,
            children: Vec::new(),
            failure: None,
            summary: None,
            details: Some(json!({"exact": true})),
        }
    }

    fn release(attempt: &str, disposition: ReleaseDisposition) -> Release {
        Release {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-a".to_string(),
            job_id: "job-a".to_string(),
            attempt_id: Some(attempt.to_string()),
            disposition,
            message: None,
        }
    }

    #[test]
    fn record_is_restart_readable_private_and_compacts_idempotently() {
        let temp = tempfile::tempdir().unwrap();
        let outbox = ResultOutbox::new(temp.path().join("results"));
        let entry = outbox.record(result("attempt-a")).unwrap();
        assert_eq!(outbox.load().unwrap(), vec![entry.clone()]);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(outbox.root()).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(outbox.entry_path(&entry.entry_id))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let ack = release("attempt-a", ReleaseDisposition::Accepted);
        assert_eq!(
            outbox.acknowledge(&entry, &ack).unwrap(),
            ResultAcknowledgement::Accepted
        );
        assert_eq!(
            outbox.acknowledge(&entry, &ack).unwrap(),
            ResultAcknowledgement::Accepted
        );
        assert!(outbox.load().unwrap().is_empty());
    }

    #[test]
    fn lost_or_mismatched_acknowledgement_retains_exact_result() {
        let temp = tempfile::tempdir().unwrap();
        let outbox = ResultOutbox::new(temp.path());
        let entry = outbox.record(result("attempt-a")).unwrap();
        assert!(
            outbox
                .acknowledge(
                    &entry,
                    &release("attempt-b", ReleaseDisposition::Superseded)
                )
                .is_err()
        );
        assert_eq!(outbox.load().unwrap(), vec![entry]);
    }

    #[test]
    fn startup_cleans_temps_and_quarantines_malformed_entries() {
        let temp = tempfile::tempdir().unwrap();
        let outbox = ResultOutbox::new(temp.path());
        outbox.prepare().unwrap();
        fs::write(outbox.pending_dir().join("left.tmp"), b"partial").unwrap();
        fs::write(outbox.pending_dir().join("broken.json"), b"{not json").unwrap();

        assert!(outbox.load().unwrap().is_empty());
        assert!(fs::read_dir(outbox.pending_dir()).unwrap().next().is_none());
        assert_eq!(fs::read_dir(outbox.quarantine_dir()).unwrap().count(), 1);
    }

    #[test]
    fn permanent_rejection_remains_operator_visible() {
        let temp = tempfile::tempdir().unwrap();
        let outbox = ResultOutbox::new(temp.path());
        let entry = outbox.record(result("attempt-a")).unwrap();
        outbox.reject(&entry, "authentication rejected").unwrap();
        assert!(outbox.load().unwrap().is_empty());
        assert!(
            outbox
                .rejected_dir()
                .join(format!("{}.json", entry.entry_id))
                .exists()
        );
    }
}
