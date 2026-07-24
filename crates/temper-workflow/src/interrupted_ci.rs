// SPDX-License-Identifier: MPL-2.0

//! Durable progress for bounded recovery of an exact interrupted CI attempt.

use crate::{CiTerminalEvidence, RoleId};
use serde::{Deserialize, Serialize};
use temper_forge::{CiRetryJobSetFingerprint, CiRetryOutcome, PullRequestId, RepositoryId};

/// Restart-safe identity and progress for one exact interrupted CI attempt.
///
/// This marker is deliberately separate from missing-current-head recovery: a
/// visible terminal attempt has provider identity and may support an exact
/// retry, while missing CI has no attempt that can safely be retried.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InterruptedCiRecoveryState {
    pub repository_id: RepositoryId,
    pub pull_request_id: PullRequestId,
    pub head_sha: String,
    pub run_id: String,
    pub attempt: String,
    pub latest_jobs: CiRetryJobSetFingerprint,
    #[serde(default)]
    pub evidence: Vec<CiTerminalEvidence>,
    /// Installed before the provider mutation. `true` without an outcome is an
    /// uncertain side-effect boundary and must never be retried blindly.
    #[serde(default)]
    pub retry_started: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_outcome: Option<CiRetryOutcome>,
    /// Selected non-code diagnostic action, when the workflow configures one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<InterruptedCiDiagnosticState>,
    /// True only when this recovery installed `needs-human` together with the
    /// marker. Superseding evidence may then remove that owned barrier without
    /// disturbing unrelated human-attention labels.
    #[serde(default)]
    pub parking_barrier_installed: bool,
}

/// Durable publication boundary for the one allowed diagnostic assignment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InterruptedCiDiagnosticState {
    pub queue: String,
    pub role: RoleId,
    pub action: String,
    /// Set atomically with durable assignment claim, before publication to a
    /// worker. Once set, absence of the assignment means the diagnostic is
    /// exhausted; duplicate observations must park rather than dispatch again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
}
