// SPDX-License-Identifier: MPL-2.0

//! Engine-owned durable storage for canonical agent activity.
//!
//! The journal deliberately has no transport concerns. Callers authenticate a
//! worker, construct an [`AuthenticatedWorkerBinding`], and then pass that
//! binding and an [`AgentActivityBatch`] to [`AgentTraceJournal::ingest`].
//! Every acknowledgement returned by the journal describes data that has been
//! appended and synced already.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, SecondsFormat, Utc};
use fs2::FileExt;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, ActivityValidationError, AgentActivityAcknowledgement,
    AgentActivityBatch, AgentActivityCapturePolicyV1, AgentActivityEventV1,
    AgentAssignmentIdentityV1, AgentRunEventV1, BlobAttachmentV1, BlobReferenceV1, CaptureModeV1,
    CapturedContentV1, DroppedEventKindV1, MAX_BLOB_ATTACHMENT_BYTES,
    MODEL_CALL_RETRY_FAILURE_MESSAGE, TraceGapV1, UsageV1, validate_run_stream,
};

use crate::{EngineAgentTraceConfig, WallClock, system_clock};

const JOURNAL_FORMAT_VERSION: u32 = 1;
const MAX_BATCH_EVENTS: usize = 10_000;
const MAX_BATCH_BLOBS: usize = 1_024;
const MAX_BATCH_ENCODED_BLOB_BYTES: u64 = 128 * 1024 * 1024;
const OMISSION_MARKER_BYTES: u64 = 1;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Configuration for one engine journal.
#[derive(Clone, Debug)]
pub struct TraceJournalConfig {
    pub root: PathBuf,
    pub policy: AgentActivityCapturePolicyV1,
}

/// Trusted identity supplied by the authenticated worker transport.
///
/// `assignment_id` is the durable assignment/attempt identity. `job_id` remains
/// part of the protocol assignment DTO because retries can preserve a job while
/// changing its durable assignment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedWorkerBinding {
    pub worker_id: String,
    pub assignment_id: String,
    pub assignment: AgentAssignmentIdentityV1,
    pub agent_session_id: Option<String>,
    pub capture_policy: AgentActivityCapturePolicyV1,
}

/// Immutable per-run binding, written exactly once as `manifest.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTraceManifest {
    pub format_version: u32,
    pub run_id: String,
    pub worker_id: String,
    pub assignment_id: String,
    pub assignment: AgentAssignmentIdentityV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    pub capture_policy: AgentActivityCapturePolicyV1,
    pub created_at: String,
}

/// One fully revalidated journal run used by the authorized query projection.
///
/// Constructing this value re-reads the append-only stream and verifies every
/// referenced blob, so query callers never serve a stale summary or an
/// unchecked content-addressed reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentTraceRun {
    pub manifest: AgentTraceManifest,
    pub summary: AgentTraceSummary,
    pub events: Vec<AgentRunEventV1>,
}

/// Terminal or partial state rebuilt entirely from readable JSONL.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTraceRunStatus {
    Active,
    Succeeded,
    Cancelled,
    Failed,
}

/// Atomic per-run projection. This file is a cache; `events.jsonl` remains the
/// authority and can rebuild every field here.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTraceSummary {
    pub format_version: u32,
    pub run_id: String,
    pub status: AgentTraceRunStatus,
    pub first_seq: Option<u64>,
    pub last_accepted_seq: u64,
    pub event_count: u64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub usage: UsageV1,
    pub dropped_events: u64,
    pub blob_count: u64,
    pub blob_bytes: u64,
    pub stored_bytes: u64,
    pub quota_exceeded_for_required_boundaries: bool,
}

/// Runs and durable assignments that a retention pass must preserve even if a
/// stale terminal summary exists. Incomplete runs are always preserved without
/// needing to appear here.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RetentionProtection {
    pub run_ids: BTreeSet<String>,
    pub assignment_ids: BTreeSet<String>,
    pub job_ids: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RetentionReport {
    pub examined: u64,
    pub removed: u64,
    pub preserved_incomplete: u64,
    pub preserved_in_flight: u64,
    pub failures: Vec<TraceRecoveryFailure>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceRecoveryReport {
    pub recovered_runs: u64,
    pub truncated_final_fragments: u64,
    pub failures: Vec<TraceRecoveryFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceRecoveryFailure {
    pub run_directory: String,
    pub error: String,
}

/// Security audit record. Captured payloads and assignment content are never
/// copied into this root-level append-only log.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceAuditRecord {
    pub format_version: u32,
    pub occurred_at: String,
    pub kind: String,
    pub run_id: String,
    pub worker_id: String,
    pub assignment_id: String,
    pub seq: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceDigestRecord {
    seq: u64,
    digest: String,
}

#[derive(Debug)]
pub enum TraceJournalError {
    Disabled,
    InvalidBinding(String),
    BindingMismatch,
    Validation(ActivityValidationError),
    PolicyViolation(String),
    SequenceGap {
        expected: u64,
        received: u64,
    },
    ConflictingRetransmit {
        seq: u64,
    },
    TerminalConsistency(String),
    CorruptRun(String),
    Io {
        operation: String,
        source: std::io::Error,
    },
    Serialization(String),
    LockPoisoned,
}

impl fmt::Display for TraceJournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("agent trace capture is disabled"),
            Self::InvalidBinding(detail) => write!(formatter, "invalid worker binding: {detail}"),
            Self::BindingMismatch => {
                formatter.write_str("run is bound to a different worker or assignment")
            }
            Self::Validation(error) => write!(formatter, "invalid activity batch: {error}"),
            Self::PolicyViolation(detail) => {
                write!(formatter, "activity violates capture policy: {detail}")
            }
            Self::SequenceGap { expected, received } => write!(
                formatter,
                "activity batch begins after a gap: expected sequence {expected}, received {received}"
            ),
            Self::ConflictingRetransmit { seq } => {
                write!(formatter, "sequence {seq} conflicts with the durable event")
            }
            Self::TerminalConsistency(detail) => {
                write!(formatter, "invalid terminal state: {detail}")
            }
            Self::CorruptRun(detail) => write!(formatter, "corrupt trace run: {detail}"),
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::Serialization(detail) => {
                write!(formatter, "trace serialization failed: {detail}")
            }
            Self::LockPoisoned => formatter.write_str("trace journal process lock is poisoned"),
        }
    }
}

impl std::error::Error for TraceJournalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<ActivityValidationError> for TraceJournalError {
    fn from(error: ActivityValidationError) -> Self {
        Self::Validation(error)
    }
}

/// A cloneable handle to the engine-owned journal.
#[derive(Clone)]
pub struct AgentTraceJournal {
    inner: Arc<JournalInner>,
}

struct JournalInner {
    root: PathBuf,
    runs_root: PathBuf,
    policy: AgentActivityCapturePolicyV1,
    clock: WallClock,
    process_lock: Mutex<()>,
    lock_file: File,
    source_digest_key: [u8; 32],
}

struct RunPaths {
    directory: PathBuf,
    manifest: PathBuf,
    events: PathBuf,
    summary: PathBuf,
    source_digests: PathBuf,
    blobs: PathBuf,
}

struct RecoveredRun {
    manifest: AgentTraceManifest,
    events: Vec<AgentRunEventV1>,
    source_digests: BTreeMap<u64, String>,
    summary: AgentTraceSummary,
}

include!("api.rs");
include!("query.rs");
include!("policy.rs");
include!("storage.rs");
include!("recovery.rs");
