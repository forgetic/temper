//! Worker-owned agent activity collection and durable local spooling.
//!
//! A [`TraceRun`] is the trust, ordering, and durable-sync boundary for one
//! invocation. It stamps [`WorkspaceContext`] identity and canonical sequence
//! numbers. [`TraceError`] lets callers degrade tracing without changing work.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityCapturePolicyV1, AgentActivityEventV1,
    AgentActivityFrameV1, AgentAssignmentIdentityV1, AgentRunEventV1, AgentScopeKindV1,
    AgentScopeV1, BlobAttachmentV1, BlobReferenceV1, CaptureModeV1, RunStartedV1,
};
use temper_protocol_agent::WorkspaceContext;
use thiserror::Error;

#[cfg(test)]
use crate::config::WorkerAgentTraceConfig;

mod accept;
mod coordination;
#[cfg(test)]
mod coordination_tests;
mod endpoint;
#[cfg(test)]
mod endpoint_tests;
mod forward;
mod forwarder;
mod model;
mod run_acknowledgement;
mod scope;
mod spool;
mod terminal;
use coordination::TraceCoordination;
pub use coordination::{DirtyTraceRun, DirtyTraceRuns, TraceCollector, TraceCoordinationSnapshot};
pub use endpoint::ActivityEndpoint;
pub(crate) use forwarder::spawn_activity_forwarder;
use model::*;
use scope::{canonicalize_child_scope, validate_scope_acceptance};
use spool::*;
pub use spool::{
    TraceReclamationReport, TraceSpoolEntry, TraceSpoolInventory, TraceSpoolOutcome,
    TraceSpoolOutcomeCounts,
};

/// Maximum encoded bytes accepted for one bare child frame.
pub const MAX_CHILD_ACTIVITY_FRAME_BYTES: usize = 256 * 1024;
/// Maximum encoded bytes accepted for one attachment-bearing child record.
///
/// This independently bounds canonical base64 for one legal 8 MiB blob plus
/// the protocol's fixed, bounded frame and envelope allowance.
pub const MAX_CHILD_ACTIVITY_RECORD_BYTES: usize =
    temper_protocol_activity::MAX_CHILD_ACTIVITY_RECORD_BYTES;
/// Aggregate worker spool capacity, expressed as full per-run reservations.
///
/// Reserving the complete `max_run_bytes` budget when a run starts guarantees
/// that concurrent runs cannot consume one another's terminal-event space.
/// Fully acknowledged terminal runs are compacted to their actual marker size,
/// making their reservation available to later work.
pub const WORKER_SPOOL_RUN_CAPACITY: u64 = 16;
const ACK_CURSOR_GROWTH_RESERVE: u64 = 32;

/// An immutable per-run spool manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceManifestV1 {
    pub version: u32,
    pub run_id: String,
    pub started_at: String,
    pub assignment: AgentAssignmentIdentityV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    pub policy: AgentActivityCapturePolicyV1,
    pub main_scope: AgentScopeV1,
}

/// A restart-readable run reconstructed from its durable spool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveredTraceRun {
    pub manifest: TraceManifestV1,
    pub events: Vec<AgentRunEventV1>,
    pub blobs: Vec<BlobAttachmentV1>,
    pub acknowledged_seq: u64,
}

/// Non-fatal activity collection/storage errors.
#[derive(Debug, Error)]
pub enum TraceError {
    #[error("{operation} {}: {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("activity JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid activity: {0}")]
    Validation(#[from] temper_protocol_activity::ActivityValidationError),
    #[error("invalid trace spool: {0}")]
    InvalidSpool(String),
    #[error("activity tracing is disabled for this run")]
    Disabled,
    #[error("activity run byte quota exceeded")]
    QuotaExceeded,
    #[error("aggregate activity spool quota of {limit} bytes exceeded")]
    AggregateQuotaExceeded { limit: u64 },
    #[error("activity run already has a terminal event")]
    AlreadyTerminal,
    #[error("acknowledgement {acknowledged} exceeds last durable sequence {last_seq}")]
    InvalidAcknowledgement { acknowledged: u64, last_seq: u64 },
}

impl TraceCollector {
    /// Starts one run. Capture `off` intentionally returns `Ok(None)` and does
    /// not create a spool directory.
    pub fn begin_run(
        &self,
        job_id: &str,
        context: &WorkspaceContext,
    ) -> Result<Option<TraceRun>, TraceError> {
        self.config.policy.validate()?;
        if self.config.policy.capture == CaptureModeV1::Off {
            return Ok(None);
        }
        let root = self.config.spool_root.as_ref().ok_or_else(|| {
            TraceError::InvalidSpool(
                "capture is enabled but no durable worker spool root is configured".to_string(),
            )
        })?;
        TraceRun::create(
            root,
            self.config.policy.clone(),
            job_id,
            context,
            Arc::clone(&self.coordination),
        )
        .map(Some)
    }

    /// Produces the deterministic classification and accounting report used by
    /// aggregate-capacity admission. The scan never follows symbolic links.
    pub fn inventory(&self) -> Result<TraceSpoolInventory, TraceError> {
        let Some(root) = self.config.spool_root.as_deref() else {
            return Ok(TraceSpoolInventory::default());
        };
        match fs::symlink_metadata(root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(TraceSpoolInventory::default());
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
        spool_inventory(root, &self.coordination)
    }

    /// Recovers every complete JSONL record, blob, and acknowledgement cursor.
    /// A non-newline-terminated final fragment is truncated in place; complete
    /// malformed records are rejected rather than silently skipped.
    pub fn recover(&self) -> Result<Vec<RecoveredTraceRun>, TraceError> {
        let Some(root) = self.config.spool_root.as_deref() else {
            return Ok(Vec::new());
        };
        if !root.exists() {
            return Ok(Vec::new());
        }
        repair_spool_root_permissions(root)?;
        let mut run_dirs = read_dir(root)?
            .filter_map(Result::ok)
            .filter_map(|entry| match entry.file_type() {
                Ok(file_type)
                    if file_type.is_dir()
                        && entry.file_name().to_str() != Some(TRACE_QUARANTINE_DIR) =>
                {
                    Some(entry.path())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        run_dirs.sort();
        run_dirs
            .iter()
            .map(|run_dir| recover_run(run_dir))
            .collect()
    }
}

/// Live collector for a single invocation.
#[derive(Clone)]
pub struct TraceRun {
    inner: Arc<TraceRunInner>,
}

struct TraceRunInner {
    manifest: TraceManifestV1,
    run_dir: PathBuf,
    blobs_dir: PathBuf,
    events_path: PathBuf,
    cursor_path: PathBuf,
    lock_path: PathBuf,
    lock_file: File,
    _ownership_file: File,
    started: Instant,
    state: Mutex<RunState>,
    coordination: Arc<TraceCoordination>,
}

impl Drop for TraceRunInner {
    fn drop(&mut self) {
        self.coordination.unregister_active(&self.manifest.run_id);
        // The ownership file remains locked until after this drop body, so a
        // scanner that observes the local registration removal still cannot
        // claim the run before the final owner is gone.
    }
}

struct RunState {
    event_file: File,
    next_seq: u64,
    used_bytes: u64,
    terminal_reserve: u64,
    acknowledged_seq: u64,
    event_end_offsets: Vec<u64>,
    terminal: Option<TraceTerminal>,
    disabled: bool,
    scopes: BTreeMap<String, AgentScopeV1>,
    /// Source root identity supplied by the first-party child. The worker maps
    /// it onto its own per-run canonical root so host boundaries and child
    /// events share exactly one unique main scope.
    source_main_scope_id: Option<String>,
    blobs: BTreeMap<String, BlobReferenceV1>,
    /// Exact attachment-bearing source frames already accepted during this
    /// live run. The attachment bytes are integrity-bound by the frame digest,
    /// so an exact retransmission returns its original sequence without
    /// consuming event or blob quota again.
    accepted_child_records: BTreeMap<Vec<u8>, u64>,
    /// A prompt is a required once-per-scope boundary. Retain its original,
    /// canonicalized source frame so both inline and blob retransmissions are
    /// idempotent while conflicting second prompts are rejected.
    accepted_prompts: BTreeMap<String, (AgentActivityFrameV1, u64)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TraceTerminal {
    sequence: u64,
    kind: TraceTerminalKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TraceTerminalKind {
    Cancelled,
    Other,
}

impl TraceRun {
    fn create(
        root: &Path,
        policy: AgentActivityCapturePolicyV1,
        job_id: &str,
        context: &WorkspaceContext,
        coordination: Arc<TraceCoordination>,
    ) -> Result<Self, TraceError> {
        create_private_dir_all(root)?;
        let (root_lock_path, root_lock_file) = open_spool_root_lock(root)?;
        root_lock_file
            .lock_exclusive()
            .map_err(|source| io_error("lock aggregate trace spool", &root_lock_path, source))?;
        let aggregate_limit = policy
            .max_run_bytes
            .saturating_mul(WORKER_SPOOL_RUN_CAPACITY);
        let aggregate_result = ensure_aggregate_spool_capacity(
            root,
            &coordination,
            policy.max_run_bytes,
            aggregate_limit,
        );
        if let Err(error) = aggregate_result {
            let _ = fs2::FileExt::unlock(&root_lock_file);
            return Err(error);
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let run_dir = root.join(&run_id);
        let creation = (|| {
            create_private_dir(&run_dir)?;
            let (lock_path, lock_file) = open_spool_lock(&run_dir)?;
            lock_file
                .lock_exclusive()
                .map_err(|source| io_error("lock new trace spool", &lock_path, source))?;
            let (ownership_path, ownership_file) = open_spool_owner_lock(&run_dir)?;
            ownership_file.lock_exclusive().map_err(|source| {
                io_error(
                    "lock new trace spool lifetime ownership",
                    &ownership_path,
                    source,
                )
            })?;
            let blobs_dir = run_dir.join("blobs");
            create_private_dir(&blobs_dir)?;

            let started_at = now_rfc3339();
            let assignment = assignment_from_context(job_id, context);
            let agent_session_id = context
                .agent_session
                .as_ref()
                .map(|session| session.session_id.clone());
            let main_scope = AgentScopeV1 {
                id: format!("main-{}", uuid::Uuid::new_v4()),
                kind: AgentScopeKindV1::Main,
                parent_id: None,
            };
            let manifest = TraceManifestV1 {
                version: ACTIVITY_PROTOCOL_VERSION,
                run_id,
                started_at: started_at.clone(),
                assignment,
                agent_session_id,
                policy,
                main_scope: main_scope.clone(),
            };
            validate_manifest(&manifest)?;

            let manifest_path = run_dir.join("manifest.json");
            let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
            atomic_write(&manifest_path, &manifest_bytes, false)?;

            let cursor_path = run_dir.join("acknowledgement.json");
            let cursor = TraceAckCursorV1::new(&manifest.run_id, 0);
            let cursor_bytes = serde_json::to_vec_pretty(&cursor)?;
            atomic_write(&cursor_path, &cursor_bytes, false)?;

            let events_path = run_dir.join("events.jsonl");
            let event_file = create_private_file(&events_path, true)?;
            let terminal_reserve = terminal_reserve_bytes(&manifest)?;
            let used_bytes = u64::try_from(manifest_bytes.len())
                .unwrap_or(u64::MAX)
                .saturating_add(u64::try_from(cursor_bytes.len()).unwrap_or(u64::MAX))
                .saturating_add(ACK_CURSOR_GROWTH_RESERVE);
            let mut scopes = BTreeMap::new();
            scopes.insert(main_scope.id.clone(), main_scope);
            let run = Self {
                inner: Arc::new(TraceRunInner {
                    manifest,
                    run_dir: run_dir.clone(),
                    blobs_dir,
                    events_path,
                    cursor_path,
                    lock_path: lock_path.clone(),
                    lock_file,
                    _ownership_file: ownership_file,
                    started: Instant::now(),
                    state: Mutex::new(RunState {
                        event_file,
                        next_seq: 1,
                        used_bytes,
                        terminal_reserve,
                        acknowledged_seq: 0,
                        event_end_offsets: Vec::new(),
                        terminal: None,
                        disabled: false,
                        scopes,
                        source_main_scope_id: None,
                        blobs: BTreeMap::new(),
                        accepted_child_records: BTreeMap::new(),
                        accepted_prompts: BTreeMap::new(),
                    }),
                    coordination,
                }),
            };
            run.inner
                .coordination
                .register_active(&run.inner.manifest.run_id);
            fs2::FileExt::unlock(&run.inner.lock_file)
                .map_err(|source| io_error("unlock new trace spool", &lock_path, source))?;
            run.append_host_event(
                started_at,
                0,
                AgentActivityEventV1::RunStarted(RunStartedV1 {
                    capture: run.inner.manifest.policy.capture,
                }),
                true,
            )?;
            Ok(run)
        })();
        let root_unlock = fs2::FileExt::unlock(&root_lock_file)
            .map_err(|source| io_error("unlock aggregate trace spool", &root_lock_path, source));
        match (creation, root_unlock) {
            (Err(error), _) => {
                let _ = std::fs::remove_dir_all(&run_dir);
                Err(error)
            }
            (Ok(_), Err(error)) => Err(error),
            (Ok(run), Ok(())) => Ok(run),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.inner.manifest.run_id
    }

    pub fn manifest(&self) -> &TraceManifestV1 {
        &self.inner.manifest
    }

    /// Binds the per-run loopback endpoint used by first-party child agents.
    /// One accepted connection carries a persistent newline-delimited record
    /// stream and may remain idle while the run is active.
    pub fn bind_endpoint(&self) -> io::Result<ActivityEndpoint> {
        ActivityEndpoint::bind(self.clone())
    }

    /// Stores a validated content-addressed blob before a frame references it.
    /// Duplicate identical blobs are idempotent and consume no additional quota.
    pub fn store_blob(&self, attachment: &BlobAttachmentV1) -> Result<(), TraceError> {
        attachment.validate()?;
        if !matches!(
            self.inner.manifest.policy.capture,
            CaptureModeV1::Transcript | CaptureModeV1::Diagnostic
        ) {
            return Err(TraceError::InvalidSpool(
                "capture policy does not permit transcript blobs".to_string(),
            ));
        }
        if attachment.blob.bytes > self.inner.manifest.policy.max_blob_bytes {
            return Err(TraceError::QuotaExceeded);
        }
        let bytes = attachment.decode()?;
        let mut state = self.inner.state.lock().expect("trace run state lock");
        ensure_accepting(&state)?;
        if let Some(existing) = state.blobs.get(&attachment.blob.digest) {
            return if existing == &attachment.blob {
                Ok(())
            } else {
                Err(TraceError::InvalidSpool(
                    "one blob digest has conflicting metadata".to_string(),
                ))
            };
        }
        ensure_quota(
            &self.inner.manifest.policy,
            state.used_bytes,
            attachment.blob.bytes,
            state.terminal_reserve,
        )?;
        let path = blob_path(&self.inner.blobs_dir, &attachment.blob)?;
        lock_spool(&self.inner)?;
        if let Err(error) = atomic_write(&path, &bytes, false) {
            let _ = unlock_spool(&self.inner);
            state.disabled = true;
            return Err(error);
        }
        unlock_spool(&self.inner)?;
        state.used_bytes = state.used_bytes.saturating_add(attachment.blob.bytes);
        state
            .blobs
            .insert(attachment.blob.digest.clone(), attachment.blob.clone());
        Ok(())
    }

    fn append_host_event(
        &self,
        occurred_at: String,
        elapsed_ms: u64,
        event: AgentActivityEventV1,
        reserve_terminal: bool,
    ) -> Result<u64, TraceError> {
        let mut state = self.inner.state.lock().expect("trace run state lock");
        let seq = state.next_seq;
        let canonical = AgentRunEventV1 {
            version: ACTIVITY_PROTOCOL_VERSION,
            run_id: self.inner.manifest.run_id.clone(),
            seq,
            occurred_at,
            elapsed_ms,
            assignment: self.inner.manifest.assignment.clone(),
            agent_session_id: self.inner.manifest.agent_session_id.clone(),
            scope: self.inner.manifest.main_scope.clone(),
            turn: None,
            event,
        };
        append_event(&self.inner, &mut state, &canonical, reserve_terminal)?;
        Ok(seq)
    }

    /// Durable directory containing this run's manifest, records, blobs, and cursor.
    pub fn spool_dir(&self) -> &Path {
        &self.inner.run_dir
    }
}

fn append_event(
    inner: &TraceRunInner,
    state: &mut RunState,
    event: &AgentRunEventV1,
    reserve_terminal: bool,
) -> Result<(), TraceError> {
    event.validate()?;
    let mut bytes = serde_json::to_vec(event)?;
    bytes.push(b'\n');
    let reserve = if reserve_terminal {
        state.terminal_reserve
    } else {
        0
    };
    ensure_quota(
        &inner.manifest.policy,
        state.used_bytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        reserve,
    )?;
    lock_spool(inner)?;
    let write_result = state
        .event_file
        .write_all(&bytes)
        .and_then(|()| sync_file_data(&state.event_file));
    if let Err(source) = write_result {
        let _ = unlock_spool(inner);
        state.disabled = true;
        return Err(io_error(
            "append and sync activity event",
            &inner.events_path,
            source,
        ));
    }
    unlock_spool(inner)?;
    state.event_end_offsets.push(
        state
            .event_end_offsets
            .last()
            .copied()
            .unwrap_or(0)
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX)),
    );
    state.used_bytes = state
        .used_bytes
        .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    state.next_seq = state.next_seq.saturating_add(1);
    state
        .scopes
        .entry(event.scope.id.clone())
        .or_insert_with(|| event.scope.clone());
    inner.coordination.publish_append(&inner.manifest.run_id);
    Ok(())
}

fn lock_spool(inner: &TraceRunInner) -> Result<(), TraceError> {
    inner
        .lock_file
        .lock_exclusive()
        .map_err(|source| io_error("lock trace spool", &inner.lock_path, source))
}

fn unlock_spool(inner: &TraceRunInner) -> Result<(), TraceError> {
    fs2::FileExt::unlock(&inner.lock_file)
        .map_err(|source| io_error("unlock trace spool", &inner.lock_path, source))
}

fn ensure_accepting(state: &RunState) -> Result<(), TraceError> {
    if state.disabled {
        Err(TraceError::Disabled)
    } else if state.terminal.is_some() {
        Err(TraceError::AlreadyTerminal)
    } else {
        Ok(())
    }
}

fn ensure_quota(
    policy: &AgentActivityCapturePolicyV1,
    used: u64,
    additional: u64,
    reserve: u64,
) -> Result<(), TraceError> {
    if used
        .checked_add(additional)
        .and_then(|total| total.checked_add(reserve))
        .is_none_or(|total| total > policy.max_run_bytes)
    {
        Err(TraceError::QuotaExceeded)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod forward_index_tests;

#[cfg(test)]
mod full_path_fixture;

#[cfg(test)]
mod full_path_model_failure_tests;

#[cfg(test)]
mod full_path_observation;

#[cfg(test)]
mod full_path_retry_tests;

#[cfg(test)]
mod full_path_tests;

#[cfg(test)]
mod inventory_tests;

#[cfg(test)]
mod reclamation_tests;

#[cfg(test)]
mod prompt_tests;

#[cfg(test)]
mod spool_idle_tests;

#[cfg(test)]
mod tests;
