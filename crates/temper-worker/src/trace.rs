//! Worker-owned agent activity collection and durable local spooling.
//!
//! A [`TraceRun`] is the trust and ordering boundary for one agent invocation:
//! it stamps assignment identity from [`WorkspaceContext`], assigns sequence
//! numbers while holding one acceptance lock, and synchronizes each JSONL record
//! before returning it to a caller. Trace failures are deliberately reported as
//! [`TraceError`] values so runners can log and degrade tracing without changing
//! the product-work outcome.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityBatch, AgentActivityCapturePolicyV1,
    AgentActivityEventV1, AgentActivityFrameV1, AgentAssignmentIdentityV1, AgentRunEventV1,
    AgentScopeKindV1, AgentScopeV1, BlobAttachmentV1, BlobReferenceV1, CaptureModeV1,
    FailureCodeV1, FailureInfoV1, RunFailedV1, RunFinishedV1, RunStartedV1, RunStatusV1,
    StopReasonV1,
};
use temper_protocol_agent::WorkspaceContext;
use thiserror::Error;

use crate::config::WorkerAgentTraceConfig;

mod endpoint;
mod model;
mod scope;
mod spool;
pub use endpoint::ActivityEndpoint;
use model::*;
use scope::{canonicalize_child_scope, validate_scope_acceptance};
use spool::*;

/// Maximum encoded bytes accepted from one child connection.
pub const MAX_CHILD_ACTIVITY_FRAME_BYTES: usize = 256 * 1024;
const ACK_CURSOR_GROWTH_RESERVE: u64 = 32;
const MAX_TERMINAL_FAILURE_MESSAGE_BYTES: usize = 512;

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

impl RecoveredTraceRun {
    /// Builds the next at-least-once forwarding batch after the durable cursor.
    /// Blob attachments are included exactly when selected events reference them.
    pub fn pending_batch(&self, max_events: usize) -> Option<AgentActivityBatch> {
        if max_events == 0 {
            return None;
        }
        let events = self
            .events
            .iter()
            .filter(|event| event.seq > self.acknowledged_seq)
            .take(max_events)
            .cloned()
            .collect::<Vec<_>>();
        let first_seq = events.first()?.seq;
        let referenced = events
            .iter()
            .flat_map(|event| event_blob_references(&event.event))
            .map(|reference| reference.digest.as_str())
            .collect::<BTreeSet<_>>();
        let blobs = self
            .blobs
            .iter()
            .filter(|attachment| referenced.contains(attachment.blob.digest.as_str()))
            .cloned()
            .collect();
        Some(AgentActivityBatch {
            version: ACTIVITY_PROTOCOL_VERSION,
            run_id: self.manifest.run_id.clone(),
            first_seq,
            events,
            blobs,
        })
    }
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
    #[error("activity run already has a terminal event")]
    AlreadyTerminal,
    #[error("acknowledgement {acknowledged} exceeds last durable sequence {last_seq}")]
    InvalidAcknowledgement { acknowledged: u64, last_seq: u64 },
}

/// Factory for new worker-stamped runs and restart recovery.
#[derive(Clone, Debug)]
pub struct TraceCollector {
    config: WorkerAgentTraceConfig,
}

impl Default for TraceCollector {
    fn default() -> Self {
        Self::new(WorkerAgentTraceConfig::default())
    }
}

impl TraceCollector {
    pub fn new(config: WorkerAgentTraceConfig) -> Self {
        Self { config }
    }

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
        TraceRun::create(root, self.config.policy.clone(), job_id, context).map(Some)
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
        set_private_dir(root)?;
        let mut run_dirs = read_dir(root)?
            .filter_map(Result::ok)
            .filter_map(|entry| match entry.file_type() {
                Ok(file_type) if file_type.is_dir() => Some(entry.path()),
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
    started: Instant,
    state: Mutex<RunState>,
}

struct RunState {
    event_file: File,
    next_seq: u64,
    used_bytes: u64,
    terminal_reserve: u64,
    acknowledged_seq: u64,
    terminal: bool,
    disabled: bool,
    scopes: BTreeMap<String, AgentScopeV1>,
    /// Source root identity supplied by the first-party child. The worker maps
    /// it onto its own per-run canonical root so host boundaries and child
    /// events share exactly one unique main scope.
    source_main_scope_id: Option<String>,
    blobs: BTreeMap<String, BlobReferenceV1>,
}

impl TraceRun {
    fn create(
        root: &Path,
        policy: AgentActivityCapturePolicyV1,
        job_id: &str,
        context: &WorkspaceContext,
    ) -> Result<Self, TraceError> {
        create_private_dir_all(root)?;
        let run_id = uuid::Uuid::new_v4().to_string();
        let run_dir = root.join(&run_id);
        create_private_dir(&run_dir)?;
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
                run_dir,
                blobs_dir,
                events_path,
                cursor_path,
                started: Instant::now(),
                state: Mutex::new(RunState {
                    event_file,
                    next_seq: 1,
                    used_bytes,
                    terminal_reserve,
                    acknowledged_seq: 0,
                    terminal: false,
                    disabled: false,
                    scopes,
                    source_main_scope_id: None,
                    blobs: BTreeMap::new(),
                }),
            }),
        };
        run.append_host_event(
            started_at,
            0,
            AgentActivityEventV1::RunStarted(RunStartedV1 {
                capture: run.inner.manifest.policy.capture,
            }),
            true,
        )?;
        Ok(run)
    }

    pub fn run_id(&self) -> &str {
        &self.inner.manifest.run_id
    }

    pub fn manifest(&self) -> &TraceManifestV1 {
        &self.inner.manifest
    }

    /// Binds the per-run loopback endpoint used by first-party child agents.
    pub fn bind_endpoint(&self) -> io::Result<ActivityEndpoint> {
        ActivityEndpoint::bind(self.clone())
    }

    /// Validates and durably accepts one untrusted child frame.
    pub fn accept_frame(&self, mut frame: AgentActivityFrameV1) -> Result<u64, TraceError> {
        frame.validate()?;
        let encoded_len = serde_json::to_vec(&frame)?.len();
        if encoded_len > MAX_CHILD_ACTIVITY_FRAME_BYTES {
            return Err(TraceError::InvalidSpool(format!(
                "child frame exceeds {MAX_CHILD_ACTIVITY_FRAME_BYTES} bytes"
            )));
        }
        let mut state = self.inner.state.lock().expect("trace run state lock");
        ensure_accepting(&state)?;
        frame.scope = canonicalize_child_scope(
            &mut state.source_main_scope_id,
            &self.inner.manifest.main_scope,
            frame.scope,
        )?;
        validate_scope_acceptance(&state.scopes, &frame.scope)?;
        validate_event_policy(&self.inner.manifest.policy, &frame.event)?;
        validate_blob_references(&state.blobs, &frame.event)?;

        let seq = state.next_seq;
        let event = AgentRunEventV1 {
            version: ACTIVITY_PROTOCOL_VERSION,
            run_id: self.inner.manifest.run_id.clone(),
            seq,
            occurred_at: frame.occurred_at,
            elapsed_ms: elapsed_ms(self.inner.started),
            assignment: self.inner.manifest.assignment.clone(),
            agent_session_id: self.inner.manifest.agent_session_id.clone(),
            scope: frame.scope,
            turn: frame.turn,
            event: frame.event,
        };
        append_event(&self.inner, &mut state, &event, true)?;
        Ok(seq)
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
        if let Err(error) = atomic_write(&path, &bytes, false) {
            state.disabled = true;
            return Err(error);
        }
        state.used_bytes = state.used_bytes.saturating_add(attachment.blob.bytes);
        state
            .blobs
            .insert(attachment.blob.digest.clone(), attachment.blob.clone());
        Ok(())
    }

    /// Writes the sole successful terminal event for the run.
    pub fn finish_success(&self, stop_reason: Option<StopReasonV1>) -> Result<u64, TraceError> {
        let duration_ms = elapsed_ms(self.inner.started);
        self.finish(AgentActivityEventV1::RunFinished(RunFinishedV1 {
            status: RunStatusV1::Succeeded,
            duration_ms,
            stop_reason,
        }))
    }

    /// Writes the sole failed/crashed terminal event for the run.
    pub fn finish_failure(
        &self,
        code: FailureCodeV1,
        message: &str,
        retryable: bool,
    ) -> Result<u64, TraceError> {
        let message = bounded_text(message, MAX_TERMINAL_FAILURE_MESSAGE_BYTES);
        self.finish(AgentActivityEventV1::RunFailed(RunFailedV1 {
            failure: FailureInfoV1 {
                code,
                message,
                retryable,
            },
        }))
    }

    /// Atomically advances the durable forwarding cursor. Retransmitted or
    /// lower acknowledgements are idempotent; a cursor beyond durable data is rejected.
    pub fn acknowledge(&self, highest_contiguous_seq: u64) -> Result<(), TraceError> {
        let mut state = self.inner.state.lock().expect("trace run state lock");
        if state.disabled {
            return Err(TraceError::Disabled);
        }
        let last_seq = state.next_seq.saturating_sub(1);
        if highest_contiguous_seq > last_seq {
            return Err(TraceError::InvalidAcknowledgement {
                acknowledged: highest_contiguous_seq,
                last_seq,
            });
        }
        if highest_contiguous_seq <= state.acknowledged_seq {
            return Ok(());
        }
        let cursor = TraceAckCursorV1::new(&self.inner.manifest.run_id, highest_contiguous_seq);
        let bytes = serde_json::to_vec_pretty(&cursor)?;
        if let Err(error) = atomic_write(&self.inner.cursor_path, &bytes, true) {
            state.disabled = true;
            return Err(error);
        }
        state.acknowledged_seq = highest_contiguous_seq;
        Ok(())
    }

    fn finish(&self, event: AgentActivityEventV1) -> Result<u64, TraceError> {
        let mut state = self.inner.state.lock().expect("trace run state lock");
        if state.terminal {
            return Err(TraceError::AlreadyTerminal);
        }
        if state.disabled {
            return Err(TraceError::Disabled);
        }
        let seq = state.next_seq;
        let canonical = AgentRunEventV1 {
            version: ACTIVITY_PROTOCOL_VERSION,
            run_id: self.inner.manifest.run_id.clone(),
            seq,
            occurred_at: now_rfc3339(),
            elapsed_ms: elapsed_ms(self.inner.started),
            assignment: self.inner.manifest.assignment.clone(),
            agent_session_id: self.inner.manifest.agent_session_id.clone(),
            scope: self.inner.manifest.main_scope.clone(),
            turn: None,
            event,
        };
        append_event(&self.inner, &mut state, &canonical, false)?;
        state.terminal = true;
        Ok(seq)
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
    let write_result = state
        .event_file
        .write_all(&bytes)
        .and_then(|()| state.event_file.sync_data());
    if let Err(source) = write_result {
        state.disabled = true;
        return Err(io_error(
            "append and sync activity event",
            &inner.events_path,
            source,
        ));
    }
    state.used_bytes = state
        .used_bytes
        .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    state.next_seq = state.next_seq.saturating_add(1);
    state
        .scopes
        .entry(event.scope.id.clone())
        .or_insert_with(|| event.scope.clone());
    Ok(())
}

fn ensure_accepting(state: &RunState) -> Result<(), TraceError> {
    if state.disabled {
        Err(TraceError::Disabled)
    } else if state.terminal {
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

fn validate_blob_references(
    blobs: &BTreeMap<String, BlobReferenceV1>,
    event: &AgentActivityEventV1,
) -> Result<(), TraceError> {
    for reference in event_blob_references(event) {
        if blobs.get(&reference.digest) != Some(reference) {
            return Err(TraceError::InvalidSpool(format!(
                "event references unstored blob {}",
                reference.digest
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "trace/tests.rs"]
mod tests;
