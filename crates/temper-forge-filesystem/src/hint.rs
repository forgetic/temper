//! Local append-only change hints for [`FilesystemForge`](crate::FilesystemForge).
//!
//! The hint log is a best-effort latency accelerator for local process-split
//! tests and development. It is not authoritative state; workers still re-read
//! the store after every wake and the poll loop remains the correctness backstop.

use crate::FilesystemForge;
use crate::errors::backend_error;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use temper_forge_model::{
    ChangeHint, ChangeKind, ChangeSource, ChangeSourceEvent, ItemNumber, PullRequest, Repository,
    RepositoryId, RepositoryPath,
};

const HINT_LOG: &str = "hints.log";
const SOURCE_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Tailing hint source returned by [`FilesystemForge::subscribe_hints`].
pub struct FilesystemHintSource {
    path: PathBuf,
    offset: u64,
    poll_interval: Duration,
}

impl FilesystemHintSource {
    fn new(path: PathBuf, offset: u64) -> Self {
        Self {
            path,
            offset,
            poll_interval: SOURCE_POLL_INTERVAL,
        }
    }

    fn read_next_hint(&mut self) -> Option<ChangeHint> {
        let metadata = fs::metadata(&self.path).ok()?;
        if metadata.len() < self.offset {
            self.offset = metadata.len();
            return None;
        }
        if metadata.len() == self.offset {
            return None;
        }

        let mut file = fs::File::open(&self.path).ok()?;
        file.seek(SeekFrom::Start(self.offset)).ok()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).ok()?;
        let content = String::from_utf8_lossy(&bytes);

        let mut consumed = 0_u64;
        for segment in content.split_inclusive('\n') {
            if !segment.ends_with('\n') {
                break;
            }
            consumed = consumed.saturating_add(segment.len() as u64);
            let line = segment.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                continue;
            }
            if let Ok(hint) = serde_json::from_str::<ChangeHint>(line) {
                self.offset = self.offset.saturating_add(consumed);
                return Some(hint);
            }
        }

        self.offset = self.offset.saturating_add(consumed);
        None
    }
}

impl ChangeSource for FilesystemHintSource {
    fn recv_timeout(&mut self, timeout: Duration) -> ChangeSourceEvent {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(hint) = self.read_next_hint() {
                return ChangeSourceEvent::Hint(hint);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return ChangeSourceEvent::Timeout;
            }
            std::thread::sleep(self.poll_interval.min(remaining));
        }
    }

    fn try_recv(&mut self) -> ChangeSourceEvent {
        self.read_next_hint()
            .map(ChangeSourceEvent::Hint)
            .unwrap_or(ChangeSourceEvent::Timeout)
    }
}

impl FilesystemForge {
    /// Subscribes to future local change hints for this filesystem store.
    ///
    /// The source starts at the current end of the append-only log, so a
    /// restarted listener may miss old hints. That is acceptable because hints
    /// only accelerate the next normal poll/tick.
    pub fn subscribe_hints(&self) -> FilesystemHintSource {
        let path = self.hint_log_path();
        let offset = fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        FilesystemHintSource::new(path, offset)
    }

    pub(crate) fn publish_path_hint(&self, path: RepositoryPath, kind: ChangeKind) {
        self.publish_hint(ChangeHint::repo(path, kind));
    }

    pub(crate) fn publish_repo_hint(&self, repo_id: &RepositoryId, kind: ChangeKind) {
        if let Ok(Some(repo)) = self.find_repository_by_id(repo_id) {
            self.publish_hint(ChangeHint::repo(repo_path(&repo), kind));
        }
    }

    pub(crate) fn publish_item_hint(
        &self,
        repo_id: &RepositoryId,
        item: ItemNumber,
        kind: ChangeKind,
    ) {
        if let Ok(Some(repo)) = self.find_repository_by_id(repo_id) {
            self.publish_hint(ChangeHint::item(repo_path(&repo), item, kind));
        }
    }

    pub(crate) fn publish_pull_request_hint(&self, pr: &PullRequest, kind: ChangeKind) {
        self.publish_item_hint(&pr.repo_id, pr.number, kind);
    }

    fn hint_log_path(&self) -> PathBuf {
        self.root().join(HINT_LOG)
    }

    fn publish_hint(&self, hint: ChangeHint) {
        let _ = self.append_hint(&hint);
    }

    fn append_hint(&self, hint: &ChangeHint) -> temper_forge_model::ForgeResult<()> {
        fs::create_dir_all(self.root()).map_err(|error| {
            backend_error(
                format!("create storage root {}", self.root().display()),
                error,
            )
        })?;
        let path = self.hint_log_path();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| backend_error(format!("open hint log {}", path.display()), error))?;
        let line = serde_json::to_string(hint).map_err(|error| {
            backend_error(format!("serialize hint log {}", path.display()), error)
        })?;
        writeln!(file, "{line}")
            .map_err(|error| backend_error(format!("append hint log {}", path.display()), error))?;
        file.flush()
            .map_err(|error| backend_error(format!("flush hint log {}", path.display()), error))
    }
}

fn repo_path(repo: &Repository) -> RepositoryPath {
    RepositoryPath::new(repo.owner.clone(), repo.name.clone())
}
