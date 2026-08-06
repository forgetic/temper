//! Structured inputs and results for codebase-memory retention.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use temper_protocol_agent::CodebaseMemoryRetentionPolicy;

/// One provider inventory record. Optional fields are retained so incomplete
/// provider metadata can be reported and force a deletion-free pass.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodebaseMemoryProjectRecord {
    pub project: Option<String>,
    pub repo_path: Option<PathBuf>,
    pub updated_at_unix_secs: Option<u64>,
    pub ownership: Option<String>,
    #[serde(default)]
    pub estimated_bytes: Option<u64>,
}

/// One bounded provider inventory page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodebaseMemoryProjectPage {
    pub cache_instance_id: Option<String>,
    pub cache_bytes: Option<u64>,
    pub projects: Vec<CodebaseMemoryProjectRecord>,
    pub next_cursor: Option<String>,
}

/// Host-only provider operations used by retention.
pub trait CodebaseMemoryMaintenanceProvider {
    fn inventory_page(
        &mut self,
        cursor: Option<&str>,
        limit: u32,
        deadline: Instant,
    ) -> Result<CodebaseMemoryProjectPage, String>;

    fn delete_project(&mut self, project: &str, deadline: Instant) -> Result<(), String>;
}

/// Canonical workspace layout facts supplied by the worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodebaseMemoryRetentionScope {
    pub workspace_root: PathBuf,
    pub roles: BTreeSet<String>,
    pub repository_dirs: BTreeSet<String>,
}

/// A record and the conservative reason for its disposition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodebaseMemoryRetentionRecordResult {
    pub project: String,
    pub repo_path: Option<PathBuf>,
    #[serde(default)]
    pub estimated_bytes: Option<u64>,
    pub reason: String,
}

/// An isolated provider deletion failure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodebaseMemoryRetentionFailure {
    pub record: CodebaseMemoryRetentionRecordResult,
    pub error: String,
}

/// Machine-filterable outcome for a maintenance pass. Free-form provider
/// diagnostics remain in the explicit report and never enter runtime logs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodebaseMemoryRetentionOutcome {
    #[default]
    SafetyNoOp,
    Disabled,
    Completed,
    PartialFailure,
    SuppressedActiveWork,
    SuppressedOverlap,
    TimedOut,
    DiscoveryFailed,
    InventoryUncertain,
}

impl CodebaseMemoryRetentionOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SafetyNoOp => "safety_no_op",
            Self::Disabled => "disabled",
            Self::Completed => "completed",
            Self::PartialFailure => "partial_failure",
            Self::SuppressedActiveWork => "suppressed_active_work",
            Self::SuppressedOverlap => "suppressed_overlap",
            Self::TimedOut => "timed_out",
            Self::DiscoveryFailed => "discovery_failed",
            Self::InventoryUncertain => "inventory_uncertain",
        }
    }
}

/// Structured result of one retention pass.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodebaseMemoryRetentionReport {
    pub cache_instance_id: Option<String>,
    pub cache_bytes: Option<u64>,
    pub inventory_complete: bool,
    pub inventory_attempted: bool,
    pub inventory_record_count: usize,
    pub inventory_duration_ms: u64,
    pub no_op_reason: Option<String>,
    pub outcome: CodebaseMemoryRetentionOutcome,
    pub policy: Option<CodebaseMemoryRetentionPolicy>,
    pub duration_ms: u64,
    pub dry_run: bool,
    pub deleted_estimated_bytes: Option<u64>,
    pub preserved: Vec<CodebaseMemoryRetentionRecordResult>,
    pub candidates: Vec<CodebaseMemoryRetentionRecordResult>,
    pub deleted: Vec<CodebaseMemoryRetentionRecordResult>,
    pub failed: Vec<CodebaseMemoryRetentionFailure>,
}

impl CodebaseMemoryRetentionReport {
    pub fn no_op(reason: impl Into<String>) -> Self {
        Self::no_op_with_outcome(reason, CodebaseMemoryRetentionOutcome::SafetyNoOp)
    }

    pub fn no_op_with_outcome(
        reason: impl Into<String>,
        outcome: CodebaseMemoryRetentionOutcome,
    ) -> Self {
        Self {
            no_op_reason: Some(reason.into()),
            outcome,
            ..Self::default()
        }
    }

    pub(super) fn with_policy(mut self, policy: CodebaseMemoryRetentionPolicy) -> Self {
        self.policy = Some(policy);
        self
    }
}
