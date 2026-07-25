// SPDX-License-Identifier: MPL-2.0

//! Durable bounded agent-session recovery stored atomically inside each
//! coordination-scoped workspace without mutating its repositories.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use temper_protocol_activity::ModelFailureV1;
use temper_protocol_agent::AgentSessionState;
use temper_protocol_worker::{SessionRecoveryActionV1, SessionRecoveryEvidenceV1};

use crate::executor::JobCancellation;
use crate::managed_effect::JoinedBlocking;
use crate::workspace::{ScopedWorkspacePathError, scoped_workspace_root};

const LEGACY_STORE_VERSION: u32 = 1;
pub const AGENT_SESSION_STORE_VERSION: u32 = 2;
const INITIAL_FAILURE_EPOCH: u32 = 1;
const RETRYABLE_SESSION_FAILURE_LIMIT: u32 = 3;
const SESSION_DIR: &str = ".temper-agent-session";
const SESSION_FILE: &str = "state.json";
const EVIDENCE_LOCATION: &str = ".temper-agent-session/state.json";

/// The single bounded predecessor retained when an active session is rotated.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriorAgentSessionRecord {
    pub session: AgentSessionState,
    pub failed_attempt_id: String,
    pub consecutive_terminal_count: u32,
    pub model_failure: ModelFailureV1,
}

/// Version-2 durable state for one coordination-scoped agent workstream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionLedger {
    pub active_session: AgentSessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_session: Option<PriorAgentSessionRecord>,
    pub failure_epoch: u32,
    pub consecutive_terminal_count: u32,
    pub rotation_consumed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_model_failure: Option<ModelFailureV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounted_attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_decision: Option<SessionRecoveryEvidenceV1>,
}

impl AgentSessionLedger {
    pub fn new(active_session: AgentSessionState) -> Self {
        Self {
            active_session,
            prior_session: None,
            failure_epoch: INITIAL_FAILURE_EPOCH,
            consecutive_terminal_count: 0,
            rotation_consumed: false,
            latest_model_failure: None,
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
            #[cfg(test)]
            fail_before_replace: false,
        })
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

    /// Loads V2 or atomically migrates a valid V1 record. Any malformed,
    /// unsupported, or mismatched record is returned as an error without a write.
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
        let ledger = match header.version {
            LEGACY_STORE_VERSION => {
                let stored: StoredAgentSessionV1 =
                    serde_json::from_slice(&bytes).map_err(|source| {
                        AgentSessionStoreError::Json {
                            path: path.clone(),
                            source,
                        }
                    })?;
                self.validate_key(&path, &stored.role, &stored.coordination_key)?;
                let ledger = AgentSessionLedger::new(stored.state);
                self.replace_ledger_sync(&ledger)?;
                ledger
            }
            AGENT_SESSION_STORE_VERSION => {
                let stored: StoredAgentSessionV2 =
                    serde_json::from_slice(&bytes).map_err(|source| {
                        AgentSessionStoreError::Json {
                            path: path.clone(),
                            source,
                        }
                    })?;
                self.validate_key(&path, &stored.role, &stored.coordination_key)?;
                self.validate_ledger(&path, &stored.ledger)?;
                stored.ledger
            }
            version => return Err(AgentSessionStoreError::UnsupportedVersion { path, version }),
        };
        Ok(Some(ledger))
    }

    /// Compatibility save that updates only the active session while retaining
    /// all recovery history in an existing valid ledger.
    pub fn save_sync(&self, state: &AgentSessionState) -> Result<(), AgentSessionStoreError> {
        let mut ledger = self
            .load_ledger_sync()?
            .unwrap_or_else(|| AgentSessionLedger::new(state.clone()));
        ledger.active_session = state.clone();
        self.replace_ledger_sync(&ledger)
    }

    pub fn save_ledger_sync(
        &self,
        ledger: &AgentSessionLedger,
    ) -> Result<(), AgentSessionStoreError> {
        self.validate_ledger(&self.path(), ledger)?;
        self.replace_ledger_sync(ledger)
    }

    /// Accounts exactly one terminal model failure and atomically persists the
    /// resulting retry, rotation, or park decision. Replaying an attempt returns
    /// the already-persisted decision without changing the ledger.
    pub fn account_model_failure_sync(
        &self,
        attempt_id: &str,
        model_failure: &ModelFailureV1,
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

        let mut diagnostic = model_failure.clone();
        diagnostic.normalize();
        let failed_session = ledger.active_session.clone();
        let failure_count = ledger
            .consecutive_terminal_count
            .checked_add(1)
            .ok_or_else(|| AgentSessionStoreError::InvalidLedger {
                path: path.clone(),
                reason: "consecutive terminal count overflow".to_string(),
            })?;
        ledger.consecutive_terminal_count = failure_count;
        ledger.latest_model_failure = Some(diagnostic.clone());

        let should_consume_session =
            !diagnostic.retryable || failure_count >= RETRYABLE_SESSION_FAILURE_LIMIT;
        let prior_session_id = ledger
            .prior_session
            .as_ref()
            .map(|prior| prior.session.session_id.clone());
        let (action, new_session_id) = if should_consume_session && !ledger.rotation_consumed {
            let new_session = AgentSessionState::new(uuid::Uuid::new_v4().to_string());
            let new_session_id = new_session.session_id.clone();
            ledger.prior_session = Some(PriorAgentSessionRecord {
                session: failed_session.clone(),
                failed_attempt_id: attempt_id.to_string(),
                consecutive_terminal_count: failure_count,
                model_failure: diagnostic.clone(),
            });
            ledger.active_session = new_session;
            ledger.consecutive_terminal_count = 0;
            ledger.rotation_consumed = true;
            (SessionRecoveryActionV1::RotateSession, Some(new_session_id))
        } else if should_consume_session {
            (SessionRecoveryActionV1::ParkForHuman, None)
        } else {
            (SessionRecoveryActionV1::RetryCurrentSession, None)
        };

        let decision = SessionRecoveryEvidenceV1 {
            attempt_id: attempt_id.to_string(),
            failure_epoch: ledger.failure_epoch,
            failure_count,
            action,
            current_session_id: failed_session.session_id,
            prior_session_id,
            new_session_id,
            evidence_location: EVIDENCE_LOCATION.to_string(),
        };
        decision
            .validate_for_attempt(Some(attempt_id))
            .map_err(|reason| AgentSessionStoreError::InvalidLedger {
                path: path.clone(),
                reason,
            })?;
        ledger.accounted_attempt_id = Some(attempt_id.to_string());
        ledger.recovery_decision = Some(decision.clone());
        self.replace_ledger_sync(&ledger)?;
        Ok(decision)
    }

    /// Marks an authoritative successful outcome as the only boundary that
    /// starts a new failure epoch. Bounded predecessor evidence is retained.
    pub fn reset_after_success_sync(&self) -> Result<AgentSessionLedger, AgentSessionStoreError> {
        let path = self.path();
        let mut ledger = self
            .load_ledger_sync()?
            .ok_or_else(|| AgentSessionStoreError::MissingLedger { path: path.clone() })?;
        ledger.failure_epoch = ledger.failure_epoch.checked_add(1).ok_or_else(|| {
            AgentSessionStoreError::InvalidLedger {
                path,
                reason: "failure epoch overflow".to_string(),
            }
        })?;
        ledger.consecutive_terminal_count = 0;
        ledger.rotation_consumed = false;
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
        self.validate_ledger(&path, ledger)?;
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
        let stored = StoredAgentSessionV2 {
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

    fn validate_ledger(
        &self,
        path: &Path,
        ledger: &AgentSessionLedger,
    ) -> Result<(), AgentSessionStoreError> {
        let invalid = |reason: String| AgentSessionStoreError::InvalidLedger {
            path: path.to_path_buf(),
            reason,
        };
        if ledger.active_session.session_id.trim().is_empty() {
            return Err(invalid("active session id must not be empty".to_string()));
        }
        if ledger.failure_epoch == 0 {
            return Err(invalid(
                "failure epoch must be greater than zero".to_string(),
            ));
        }
        if ledger.rotation_consumed && ledger.prior_session.is_none() {
            return Err(invalid(
                "a consumed rotation requires a prior-session record".to_string(),
            ));
        }
        if let Some(diagnostic) = &ledger.latest_model_failure {
            diagnostic
                .validate()
                .map_err(|error| invalid(error.to_string()))?;
        }
        if let Some(prior) = &ledger.prior_session {
            if prior.session.session_id.trim().is_empty()
                || prior.failed_attempt_id.trim().is_empty()
                || prior.consecutive_terminal_count == 0
            {
                return Err(invalid("prior-session record is incomplete".to_string()));
            }
            prior
                .model_failure
                .validate()
                .map_err(|error| invalid(error.to_string()))?;
        }
        match (&ledger.accounted_attempt_id, &ledger.recovery_decision) {
            (None, None) => {}
            (Some(attempt_id), Some(decision)) => {
                decision
                    .validate_for_attempt(Some(attempt_id))
                    .map_err(invalid)?;
                match decision.action {
                    SessionRecoveryActionV1::RotateSession => {
                        let prior = ledger.prior_session.as_ref().ok_or_else(|| {
                            invalid("rotation decision has no prior-session record".to_string())
                        })?;
                        if prior.session.session_id != decision.current_session_id
                            || decision.new_session_id.as_deref()
                                != Some(ledger.active_session.session_id.as_str())
                            || prior.consecutive_terminal_count != decision.failure_count
                            || ledger.consecutive_terminal_count != 0
                            || !ledger.rotation_consumed
                        {
                            return Err(invalid(
                                "rotation decision does not match the persisted boundary"
                                    .to_string(),
                            ));
                        }
                    }
                    SessionRecoveryActionV1::RetryCurrentSession
                    | SessionRecoveryActionV1::ParkForHuman => {
                        if ledger.active_session.session_id != decision.current_session_id
                            || ledger.consecutive_terminal_count != decision.failure_count
                        {
                            return Err(invalid(
                                "recovery decision does not match the active session".to_string(),
                            ));
                        }
                    }
                }
            }
            _ => {
                return Err(invalid(
                    "accounted attempt and recovery decision must be present together".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct StoredVersion {
    version: u32,
}

#[derive(Deserialize)]
struct StoredAgentSessionV1 {
    #[serde(rename = "version")]
    _version: u32,
    role: String,
    coordination_key: String,
    state: AgentSessionState,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAgentSessionV2 {
    version: u32,
    role: String,
    coordination_key: String,
    ledger: AgentSessionLedger,
}

#[cfg(test)]
mod tests;
