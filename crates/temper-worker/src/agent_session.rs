// SPDX-License-Identifier: MPL-2.0

//! Durable bounded agent-session recovery stored atomically inside each
//! coordination-scoped workspace without mutating its repositories.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use temper_protocol_activity::{ModelFailureDispositionV1, ModelFailureV1};
use temper_protocol_agent::AgentSessionState;
use temper_protocol_worker::{SessionRecoveryActionV1, SessionRecoveryEvidenceV1};

use crate::executor::JobCancellation;
use crate::managed_effect::JoinedBlocking;
use crate::workspace::{ScopedWorkspacePathError, scoped_workspace_root};

const LEGACY_STORE_VERSION: u32 = 1;
const PREVIOUS_STORE_VERSION: u32 = 2;
pub const AGENT_SESSION_STORE_VERSION: u32 = 3;
const INITIAL_FAILURE_EPOCH: u32 = 1;
const LEGACY_SESSION_FAILURE_LIMIT: u32 = 3;
const SESSION_DIR: &str = ".temper-agent-session";
const SESSION_FILE: &str = "state.json";
const EVIDENCE_LOCATION: &str = ".temper-agent-session/state.json";

/// Worker-owned limits snapshotted into each unsucceeded failure epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecoveryPolicy {
    /// Terminal agent runs allowed in one durable session.
    pub session_failure_limit: u32,
    /// Fresh sessions allowed before provider deferral.
    pub fresh_session_limit: u32,
    /// Provider deferral generations allowed before human parking.
    pub provider_deferral_limit: u32,
    /// Delay selected for each provider deferral.
    pub provider_deferral_delay_ms: u64,
    /// Wall-clock SLO for the complete unsucceeded failure epoch.
    pub recovery_slo_ms: u64,
}

impl Default for SessionRecoveryPolicy {
    fn default() -> Self {
        Self {
            // Same-turn retries are now exhausted inside the agent. A terminal
            // eligible failure therefore consumes this session by default.
            session_failure_limit: 1,
            fresh_session_limit: 1,
            provider_deferral_limit: 3,
            provider_deferral_delay_ms: 300_000,
            recovery_slo_ms: 7_200_000,
        }
    }
}

impl SessionRecoveryPolicy {
    fn validate(self) -> Result<(), &'static str> {
        if self.session_failure_limit == 0 || self.session_failure_limit > 32 {
            return Err("session_failure_limit must be between 1 and 32");
        }
        if self.fresh_session_limit > 32 {
            return Err("fresh_session_limit must be at most 32");
        }
        if self.provider_deferral_limit == 0 || self.provider_deferral_limit > 32 {
            return Err("provider_deferral_limit must be between 1 and 32");
        }
        if self.provider_deferral_delay_ms == 0 {
            return Err("provider_deferral_delay_ms must be greater than zero");
        }
        if self.recovery_slo_ms == 0 {
            return Err("recovery_slo_ms must be greater than zero");
        }
        if self.provider_deferral_delay_ms > self.recovery_slo_ms {
            return Err("provider_deferral_delay_ms must not exceed recovery_slo_ms");
        }
        Ok(())
    }
}

/// The single bounded predecessor retained when an active session is rotated.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriorAgentSessionRecord {
    pub session: AgentSessionState,
    pub session_number: u32,
    pub failed_attempt_id: String,
    pub session_terminal_failures: u32,
    pub cumulative_terminal_failures: u32,
    pub model_failure: ModelFailureV1,
}

/// Version-3 durable state for one coordination-scoped agent workstream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionLedger {
    pub active_session: AgentSessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_session: Option<PriorAgentSessionRecord>,
    pub failure_epoch: u32,
    pub cumulative_terminal_failures: u32,
    pub current_session_number: u32,
    pub session_terminal_failures: u32,
    pub fresh_sessions_used: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_epoch_started_unix_ms: Option<u64>,
    pub failure_epoch_elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_slo_deadline_unix_ms: Option<u64>,
    pub recovery_policy: SessionRecoveryPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_model_failure: Option<ModelFailureV1>,
    pub immediate_retry_exhausted: bool,
    pub deferral_count: u32,
    pub deferral_generation: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounted_attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_decision: Option<SessionRecoveryEvidenceV1>,
}

impl AgentSessionLedger {
    pub fn new(active_session: AgentSessionState) -> Self {
        Self::new_with_policy(active_session, SessionRecoveryPolicy::default())
    }

    pub fn new_with_policy(
        active_session: AgentSessionState,
        recovery_policy: SessionRecoveryPolicy,
    ) -> Self {
        Self {
            active_session,
            prior_session: None,
            failure_epoch: INITIAL_FAILURE_EPOCH,
            cumulative_terminal_failures: 0,
            current_session_number: 1,
            session_terminal_failures: 0,
            fresh_sessions_used: 0,
            failure_epoch_started_unix_ms: None,
            failure_epoch_elapsed_ms: 0,
            configured_slo_deadline_unix_ms: None,
            recovery_policy,
            latest_model_failure: None,
            immediate_retry_exhausted: false,
            deferral_count: 0,
            deferral_generation: 0,
            not_before_unix_ms: None,
            accounted_attempt_id: None,
            recovery_decision: None,
        }
    }
}

/// File-backed session store for one `(role, coordination_key)` workstream.
#[derive(Clone, Debug)]
pub struct AgentSessionStore {
    role: String,
    coordination_key: String,
    workstream_root: PathBuf,
    recovery_policy: SessionRecoveryPolicy,
    #[cfg(test)]
    fail_before_replace: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentSessionStoreError {
    #[error("coordination key must not be empty")]
    EmptyCoordinationKey,
    #[error(transparent)]
    UnsafePath(#[from] ScopedWorkspacePathError),
    #[error("io error for agent session `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("agent session operation cancelled by the worker watchdog")]
    Cancelled,
    #[error("invalid agent session JSON `{path}`: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported agent session store version {version} in `{path}`")]
    UnsupportedVersion { path: PathBuf, version: u32 },
    #[error(
        "agent session key mismatch in `{path}`: expected ({expected_role}, {expected_key}), found ({found_role}, {found_key})"
    )]
    KeyMismatch {
        path: PathBuf,
        expected_role: String,
        expected_key: String,
        found_role: String,
        found_key: String,
    },
    #[error("invalid agent session ledger `{path}`: {reason}")]
    InvalidLedger { path: PathBuf, reason: String },
    #[error("agent session ledger is missing from `{path}`")]
    MissingLedger { path: PathBuf },
}

impl AgentSessionStore {
    /// Builds the session store under the same safe path component logic as
    /// coordination-scoped checkouts.
    pub fn for_workspace_root(
        workspace_root: &Path,
        role: &str,
        coordination_key: &str,
    ) -> Result<Self, AgentSessionStoreError> {
        let coordination_key = coordination_key.trim();
        if coordination_key.is_empty() {
            return Err(AgentSessionStoreError::EmptyCoordinationKey);
        }
        let workstream_root = scoped_workspace_root(workspace_root, role, coordination_key)?;
        Ok(Self {
            role: role.to_string(),
            coordination_key: coordination_key.to_string(),
            workstream_root,
            recovery_policy: SessionRecoveryPolicy::default(),
            #[cfg(test)]
            fail_before_replace: false,
        })
    }

    /// Installs validated runtime recovery limits. The selected limits are
    /// snapshotted only when a new failure epoch starts; an active epoch keeps
    /// its original policy across daemon and worker replacement.
    #[must_use]
    pub fn with_recovery_policy(mut self, recovery_policy: SessionRecoveryPolicy) -> Self {
        self.recovery_policy = recovery_policy;
        self
    }

    /// Path to the durable ledger. Exposed for diagnostics and corruption tests.
    pub fn path(&self) -> PathBuf {
        self.workstream_root.join(SESSION_DIR).join(SESSION_FILE)
    }

    #[cfg(test)]
    pub(crate) fn with_replace_failure(mut self) -> Self {
        self.fail_before_replace = true;
        self
    }

    /// Compatibility view of the active session.
    pub fn load_sync(&self) -> Result<Option<AgentSessionState>, AgentSessionStoreError> {
        Ok(self.load_ledger_sync()?.map(|ledger| ledger.active_session))
    }

    /// Loads V3 or atomically migrates a valid V1/V2 record. Any malformed,
    /// unsupported, corrupt, or mismatched record is returned without a write.
    pub fn load_ledger_sync(&self) -> Result<Option<AgentSessionLedger>, AgentSessionStoreError> {
        let path = self.path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(AgentSessionStoreError::Io { path, source }),
        };
        let header: StoredVersion =
            serde_json::from_slice(&bytes).map_err(|source| AgentSessionStoreError::Json {
                path: path.clone(),
                source,
            })?;
        let (ledger, migrate) = match header.version {
            LEGACY_STORE_VERSION => {
                let stored: StoredAgentSessionV1 =
                    serde_json::from_slice(&bytes).map_err(|source| {
                        AgentSessionStoreError::Json {
                            path: path.clone(),
                            source,
                        }
                    })?;
                self.validate_key(&path, &stored.role, &stored.coordination_key)?;
                (
                    AgentSessionLedger::new_with_policy(stored.state, self.recovery_policy),
                    true,
                )
            }
            PREVIOUS_STORE_VERSION => {
                let stored: StoredAgentSessionV2 =
                    serde_json::from_slice(&bytes).map_err(|source| {
                        AgentSessionStoreError::Json {
                            path: path.clone(),
                            source,
                        }
                    })?;
                self.validate_key(&path, &stored.role, &stored.coordination_key)?;
                validation::validate_v2_ledger(&path, &stored.ledger)?;
                (
                    migrate_v2_ledger(&path, stored.ledger, self.recovery_policy)?,
                    true,
                )
            }
            AGENT_SESSION_STORE_VERSION => {
                let stored: StoredAgentSessionV3 =
                    serde_json::from_slice(&bytes).map_err(|source| {
                        AgentSessionStoreError::Json {
                            path: path.clone(),
                            source,
                        }
                    })?;
                self.validate_key(&path, &stored.role, &stored.coordination_key)?;
                validation::validate_ledger(&path, &stored.ledger)?;
                (stored.ledger, false)
            }
            version => return Err(AgentSessionStoreError::UnsupportedVersion { path, version }),
        };
        if migrate {
            self.replace_ledger_sync(&ledger)?;
        }
        Ok(Some(ledger))
    }

    /// Compatibility save that updates only the active session while retaining
    /// all recovery history in an existing valid ledger.
    pub fn save_sync(&self, state: &AgentSessionState) -> Result<(), AgentSessionStoreError> {
        let mut ledger = self.load_ledger_sync()?.unwrap_or_else(|| {
            AgentSessionLedger::new_with_policy(state.clone(), self.recovery_policy)
        });
        ledger.active_session = state.clone();
        self.replace_ledger_sync(&ledger)
    }

    pub fn save_ledger_sync(
        &self,
        ledger: &AgentSessionLedger,
    ) -> Result<(), AgentSessionStoreError> {
        validation::validate_ledger(&self.path(), ledger)?;
        self.replace_ledger_sync(ledger)
    }

    /// Accounts exactly one terminal model failure and atomically persists the
    /// resulting retry, rotation, provider deferral, or human-park decision.
    /// Replaying an attempt returns byte-equivalent evidence without mutation.
    pub fn account_model_failure_sync(
        &self,
        attempt_id: &str,
        model_failure: &ModelFailureV1,
    ) -> Result<SessionRecoveryEvidenceV1, AgentSessionStoreError> {
        self.account_model_failure_at_sync(attempt_id, model_failure, system_time_unix_ms()?)
    }

    fn account_model_failure_at_sync(
        &self,
        attempt_id: &str,
        model_failure: &ModelFailureV1,
        now_unix_ms: u64,
    ) -> Result<SessionRecoveryEvidenceV1, AgentSessionStoreError> {
        let path = self.path();
        let mut ledger = self
            .load_ledger_sync()?
            .ok_or_else(|| AgentSessionStoreError::MissingLedger { path: path.clone() })?;

        if ledger.accounted_attempt_id.as_deref() == Some(attempt_id) {
            return ledger.recovery_decision.clone().ok_or_else(|| {
                AgentSessionStoreError::InvalidLedger {
                    path,
                    reason: "accounted attempt has no persisted recovery decision".to_string(),
                }
            });
        }
        self.recovery_policy
            .validate()
            .map_err(|reason| invalid(&path, reason))?;

        let mut diagnostic = model_failure.clone();
        diagnostic.normalize();
        diagnostic
            .validate()
            .map_err(|error| invalid(&path, &error.to_string()))?;
        let failed_session = ledger.active_session.clone();

        if ledger.cumulative_terminal_failures == 0 {
            ledger.recovery_policy = self.recovery_policy;
            ledger.failure_epoch_started_unix_ms = Some(now_unix_ms);
            ledger.configured_slo_deadline_unix_ms = Some(
                now_unix_ms
                    .checked_add(ledger.recovery_policy.recovery_slo_ms)
                    .ok_or_else(|| invalid(&path, "configured SLO deadline overflow"))?,
            );
        }
        let started = ledger
            .failure_epoch_started_unix_ms
            .ok_or_else(|| invalid(&path, "failure epoch has no start time"))?;
        let deadline = ledger
            .configured_slo_deadline_unix_ms
            .ok_or_else(|| invalid(&path, "failure epoch has no SLO deadline"))?;
        ledger.failure_epoch_elapsed_ms = now_unix_ms
            .checked_sub(started)
            .ok_or_else(|| invalid(&path, "wall clock precedes failure epoch start"))?;
        ledger.cumulative_terminal_failures = ledger
            .cumulative_terminal_failures
            .checked_add(1)
            .ok_or_else(|| invalid(&path, "cumulative terminal failure count overflow"))?;
        ledger.session_terminal_failures = ledger
            .session_terminal_failures
            .checked_add(1)
            .ok_or_else(|| invalid(&path, "session terminal failure count overflow"))?;
        ledger.latest_model_failure = Some(diagnostic.clone());
        ledger.immediate_retry_exhausted = matches!(
            diagnostic.disposition,
            ModelFailureDispositionV1::Retryable | ModelFailureDispositionV1::Unknown
        );
        ledger.not_before_unix_ms = None;

        let prior_session_id = ledger
            .prior_session
            .as_ref()
            .map(|prior| prior.session.session_id.clone());
        let actionable = diagnostic.disposition == ModelFailureDispositionV1::NonRetryable;
        let session_exhausted =
            ledger.session_terminal_failures >= ledger.recovery_policy.session_failure_limit;
        let slo_exhausted = now_unix_ms >= deadline;

        let (action, new_session_id) = if actionable || slo_exhausted {
            // Authentication, entitlement, context, and deterministic request
            // failures have concrete operator action; a fresh session cannot
            // repair them, so do not consume one pointlessly. The epoch SLO is
            // also an absolute boundary: once elapsed, no retry or rotation may
            // extend automatic recovery.
            (SessionRecoveryActionV1::ParkForHuman, None)
        } else if !session_exhausted {
            (SessionRecoveryActionV1::RetryCurrentSession, None)
        } else if ledger.fresh_sessions_used < ledger.recovery_policy.fresh_session_limit {
            let new_session = AgentSessionState::new(uuid::Uuid::new_v4().to_string());
            let new_session_id = new_session.session_id.clone();
            ledger.prior_session = Some(PriorAgentSessionRecord {
                session: failed_session.clone(),
                session_number: ledger.current_session_number,
                failed_attempt_id: attempt_id.to_string(),
                session_terminal_failures: ledger.session_terminal_failures,
                cumulative_terminal_failures: ledger.cumulative_terminal_failures,
                model_failure: diagnostic.clone(),
            });
            ledger.active_session = new_session;
            ledger.current_session_number = ledger
                .current_session_number
                .checked_add(1)
                .ok_or_else(|| invalid(&path, "session number overflow"))?;
            ledger.fresh_sessions_used = ledger
                .fresh_sessions_used
                .checked_add(1)
                .ok_or_else(|| invalid(&path, "fresh session count overflow"))?;
            ledger.session_terminal_failures = 0;
            (SessionRecoveryActionV1::RotateSession, Some(new_session_id))
        } else if ledger.deferral_count < ledger.recovery_policy.provider_deferral_limit {
            ledger.deferral_count = ledger
                .deferral_count
                .checked_add(1)
                .ok_or_else(|| invalid(&path, "deferral count overflow"))?;
            ledger.deferral_generation = ledger
                .deferral_generation
                .checked_add(1)
                .ok_or_else(|| invalid(&path, "deferral generation overflow"))?;
            let selected = now_unix_ms
                .checked_add(ledger.recovery_policy.provider_deferral_delay_ms)
                .ok_or_else(|| invalid(&path, "provider deferral not-before overflow"))?;
            ledger.not_before_unix_ms = Some(selected.min(deadline));
            (SessionRecoveryActionV1::ProviderDeferred, None)
        } else {
            (SessionRecoveryActionV1::ParkForHuman, None)
        };

        let decision = SessionRecoveryEvidenceV1 {
            attempt_id: attempt_id.to_string(),
            failure_epoch: ledger.failure_epoch,
            failure_count: ledger.cumulative_terminal_failures,
            session_number: if action == SessionRecoveryActionV1::RotateSession {
                ledger.current_session_number - 1
            } else {
                ledger.current_session_number
            },
            session_failure_count: if action == SessionRecoveryActionV1::RotateSession {
                ledger
                    .prior_session
                    .as_ref()
                    .expect("rotation just retained its predecessor")
                    .session_terminal_failures
            } else {
                ledger.session_terminal_failures
            },
            epoch_started_unix_ms: Some(started),
            epoch_elapsed_ms: ledger.failure_epoch_elapsed_ms,
            disposition: Some(diagnostic.disposition),
            immediate_retry_exhausted: ledger.immediate_retry_exhausted,
            configured_session_failure_limit: ledger.recovery_policy.session_failure_limit,
            configured_fresh_session_limit: ledger.recovery_policy.fresh_session_limit,
            configured_deferral_limit: ledger.recovery_policy.provider_deferral_limit,
            deferral_count: ledger.deferral_count,
            deferral_generation: ledger.deferral_generation,
            not_before_unix_ms: ledger.not_before_unix_ms,
            slo_deadline_unix_ms: Some(deadline),
            action,
            current_session_id: failed_session.session_id,
            prior_session_id,
            new_session_id,
            evidence_location: EVIDENCE_LOCATION.to_string(),
        };
        decision
            .validate_for_attempt(Some(attempt_id))
            .map_err(|reason| invalid(&path, &reason))?;
        ledger.accounted_attempt_id = Some(attempt_id.to_string());
        ledger.recovery_decision = Some(decision.clone());
        self.replace_ledger_sync(&ledger)?;
        Ok(decision)
    }

    /// Marks an authoritative successful outcome as the only boundary that
    /// clears deferral and starts a new failure epoch. Bounded predecessor
    /// evidence remains available, but cumulative display state is reset.
    pub fn reset_after_success_sync(&self) -> Result<AgentSessionLedger, AgentSessionStoreError> {
        let path = self.path();
        let mut ledger = self
            .load_ledger_sync()?
            .ok_or_else(|| AgentSessionStoreError::MissingLedger { path: path.clone() })?;
        ledger.failure_epoch = ledger
            .failure_epoch
            .checked_add(1)
            .ok_or_else(|| invalid(&path, "failure epoch overflow"))?;
        ledger.cumulative_terminal_failures = 0;
        ledger.current_session_number = 1;
        ledger.session_terminal_failures = 0;
        ledger.fresh_sessions_used = 0;
        ledger.failure_epoch_started_unix_ms = None;
        ledger.failure_epoch_elapsed_ms = 0;
        ledger.configured_slo_deadline_unix_ms = None;
        ledger.latest_model_failure = None;
        ledger.immediate_retry_exhausted = false;
        ledger.deferral_count = 0;
        ledger.deferral_generation = 0;
        ledger.not_before_unix_ms = None;
        ledger.accounted_attempt_id = None;
        ledger.recovery_decision = None;
        self.replace_ledger_sync(&ledger)?;
        Ok(ledger)
    }

    pub fn delete_sync(&self) -> Result<bool, AgentSessionStoreError> {
        let path = self.path();
        match std::fs::remove_file(&path) {
            Ok(()) => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::remove_dir(parent);
                }
                Ok(true)
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(AgentSessionStoreError::Io { path, source }),
        }
    }

    pub async fn load(&self) -> Result<Option<AgentSessionState>, AgentSessionStoreError> {
        self.load_controlled(&JobCancellation::default()).await
    }

    pub(crate) async fn load_controlled(
        &self,
        cancellation: &JobCancellation,
    ) -> Result<Option<AgentSessionState>, AgentSessionStoreError> {
        Ok(self
            .load_ledger_controlled(cancellation)
            .await?
            .map(|ledger| ledger.active_session))
    }

    pub(crate) async fn load_ledger_controlled(
        &self,
        cancellation: &JobCancellation,
    ) -> Result<Option<AgentSessionLedger>, AgentSessionStoreError> {
        let store = self.clone();
        let path = self.path();
        let owner = JoinedBlocking::spawn("temper-agent-session-load", move || {
            store.load_ledger_sync()
        });
        cancellation
            .run(owner)
            .await
            .ok_or(AgentSessionStoreError::Cancelled)?
            .map_err(|source| AgentSessionStoreError::Io { path, source })?
    }

    pub async fn save(&self, state: &AgentSessionState) -> Result<(), AgentSessionStoreError> {
        self.save_controlled(state, &JobCancellation::default())
            .await
    }

    pub(crate) async fn save_controlled(
        &self,
        state: &AgentSessionState,
        cancellation: &JobCancellation,
    ) -> Result<(), AgentSessionStoreError> {
        let store = self.clone();
        let state = state.clone();
        let path = self.path();
        let owner =
            JoinedBlocking::spawn("temper-agent-session-save", move || store.save_sync(&state));
        cancellation
            .run(owner)
            .await
            .ok_or(AgentSessionStoreError::Cancelled)?
            .map_err(|source| AgentSessionStoreError::Io { path, source })?
    }

    pub(crate) async fn save_ledger_controlled(
        &self,
        ledger: &AgentSessionLedger,
        cancellation: &JobCancellation,
    ) -> Result<(), AgentSessionStoreError> {
        let store = self.clone();
        let ledger = ledger.clone();
        let path = self.path();
        let owner = JoinedBlocking::spawn("temper-agent-session-ledger-save", move || {
            store.save_ledger_sync(&ledger)
        });
        cancellation
            .run(owner)
            .await
            .ok_or(AgentSessionStoreError::Cancelled)?
            .map_err(|source| AgentSessionStoreError::Io { path, source })?
    }

    pub(crate) async fn account_model_failure_controlled(
        &self,
        attempt_id: &str,
        model_failure: &ModelFailureV1,
        cancellation: &JobCancellation,
    ) -> Result<SessionRecoveryEvidenceV1, AgentSessionStoreError> {
        if cancellation.is_cancelled() {
            return Err(AgentSessionStoreError::Cancelled);
        }
        let store = self.clone();
        let attempt_id = attempt_id.to_string();
        let model_failure = model_failure.clone();
        let path = self.path();
        let owner = JoinedBlocking::spawn("temper-agent-session-account-failure", move || {
            store.account_model_failure_sync(&attempt_id, &model_failure)
        });
        cancellation
            .run(owner)
            .await
            .ok_or(AgentSessionStoreError::Cancelled)?
            .map_err(|source| AgentSessionStoreError::Io { path, source })?
    }

    pub(crate) async fn reset_after_success_controlled(
        &self,
        cancellation: &JobCancellation,
    ) -> Result<AgentSessionLedger, AgentSessionStoreError> {
        let store = self.clone();
        let path = self.path();
        let owner = JoinedBlocking::spawn("temper-agent-session-success-reset", move || {
            store.reset_after_success_sync()
        });
        cancellation
            .run(owner)
            .await
            .ok_or(AgentSessionStoreError::Cancelled)?
            .map_err(|source| AgentSessionStoreError::Io { path, source })?
    }

    #[allow(dead_code)]
    pub async fn delete(&self) -> Result<bool, AgentSessionStoreError> {
        let store = self.clone();
        let path = self.path();
        JoinedBlocking::spawn("temper-agent-session-delete", move || store.delete_sync())
            .await
            .map_err(|source| AgentSessionStoreError::Io { path, source })?
    }

    fn replace_ledger_sync(
        &self,
        ledger: &AgentSessionLedger,
    ) -> Result<(), AgentSessionStoreError> {
        let path = self.path();
        validation::validate_ledger(&path, ledger)?;
        let Some(parent) = path.parent() else {
            return Err(AgentSessionStoreError::Io {
                path,
                source: std::io::Error::other("agent session path has no parent"),
            });
        };
        std::fs::create_dir_all(parent).map_err(|source| AgentSessionStoreError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let stored = StoredAgentSessionV3 {
            version: AGENT_SESSION_STORE_VERSION,
            role: self.role.clone(),
            coordination_key: self.coordination_key.clone(),
            ledger: ledger.clone(),
        };
        let bytes =
            serde_json::to_vec_pretty(&stored).map_err(|source| AgentSessionStoreError::Json {
                path: path.clone(),
                source,
            })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|source| {
            AgentSessionStoreError::Io {
                path: parent.to_path_buf(),
                source,
            }
        })?;
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|source| AgentSessionStoreError::Io {
                path: temporary.path().to_path_buf(),
                source,
            })?;
        #[cfg(test)]
        if self.fail_before_replace {
            return Err(AgentSessionStoreError::Io {
                path,
                source: std::io::Error::other("injected atomic replacement failure"),
            });
        }
        temporary
            .persist(&path)
            .map_err(|error| AgentSessionStoreError::Io {
                path: path.clone(),
                source: error.error,
            })?;
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| AgentSessionStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        Ok(())
    }

    fn validate_key(
        &self,
        path: &Path,
        role: &str,
        coordination_key: &str,
    ) -> Result<(), AgentSessionStoreError> {
        if role != self.role || coordination_key != self.coordination_key {
            return Err(AgentSessionStoreError::KeyMismatch {
                path: path.to_path_buf(),
                expected_role: self.role.clone(),
                expected_key: self.coordination_key.clone(),
                found_role: role.to_string(),
                found_key: coordination_key.to_string(),
            });
        }
        Ok(())
    }
}

fn system_time_unix_ms() -> Result<u64, AgentSessionStoreError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| AgentSessionStoreError::Io {
            path: PathBuf::from(EVIDENCE_LOCATION),
            source: std::io::Error::other(source),
        })?
        .as_millis();
    u64::try_from(millis).map_err(|_| AgentSessionStoreError::Io {
        path: PathBuf::from(EVIDENCE_LOCATION),
        source: std::io::Error::other("system clock does not fit recovery evidence"),
    })
}

fn invalid(path: &Path, reason: &str) -> AgentSessionStoreError {
    AgentSessionStoreError::InvalidLedger {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
}

mod migration;
use migration::{
    AgentSessionLedgerV2, StoredAgentSessionV1, StoredAgentSessionV2, StoredAgentSessionV3,
    StoredVersion, migrate_v2_ledger,
};

mod validation;

#[cfg(test)]
mod tests;
