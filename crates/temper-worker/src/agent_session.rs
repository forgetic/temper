// SPDX-License-Identifier: MPL-2.0

//! Durable agent-session state keyed by a role + coordination key.
//!
//! The store lives inside the coordination-scoped workspace root:
//! `<workspace_root>/<role>/<safe-key>/.temper-agent-session/state.json`. That
//! keeps session lifetime aligned with the workstream checkout: a PR feedback job
//! for the same `(role, coordination_key)` can resume the saved state after the
//! worker slot was released, and the existing landed-PR workspace cleanup removes
//! the session state at the same time as the checkout.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use temper_protocol_agent::AgentSessionState;

use crate::executor::JobCancellation;
use crate::managed_effect::JoinedBlocking;
use crate::workspace::{ScopedWorkspacePathError, scoped_workspace_root};

const STORE_VERSION: u32 = 1;
const SESSION_DIR: &str = ".temper-agent-session";
const SESSION_FILE: &str = "state.json";

/// File-backed session store for one `(role, coordination_key)` workstream.
#[derive(Clone, Debug)]
pub struct AgentSessionStore {
    role: String,
    coordination_key: String,
    workstream_root: PathBuf,
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
}

impl AgentSessionStore {
    /// Builds the session store for a role + coordination key under the same
    /// safe path component logic as coordination-scoped checkouts.
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
        })
    }

    /// Path to the JSON state file. Exposed for deterministic tests and
    /// diagnostics; callers should use [`save_sync`](Self::save_sync) rather than
    /// writing arbitrary contents except in corruption tests.
    pub fn path(&self) -> PathBuf {
        self.workstream_root.join(SESSION_DIR).join(SESSION_FILE)
    }

    pub fn load_sync(&self) -> Result<Option<AgentSessionState>, AgentSessionStoreError> {
        let path = self.path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(AgentSessionStoreError::Io { path, source }),
        };
        let stored: StoredAgentSession =
            serde_json::from_slice(&bytes).map_err(|source| AgentSessionStoreError::Json {
                path: path.clone(),
                source,
            })?;
        self.validate_loaded(&path, &stored)?;
        Ok(Some(stored.state))
    }

    pub fn save_sync(&self, state: &AgentSessionState) -> Result<(), AgentSessionStoreError> {
        let path = self.path();
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
        let stored = StoredAgentSession {
            version: STORE_VERSION,
            role: self.role.clone(),
            coordination_key: self.coordination_key.clone(),
            state: state.clone(),
        };
        let bytes =
            serde_json::to_vec_pretty(&stored).map_err(|source| AgentSessionStoreError::Json {
                path: path.clone(),
                source,
            })?;
        let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
        std::fs::write(&tmp, bytes).map_err(|source| AgentSessionStoreError::Io {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &path).map_err(|source| AgentSessionStoreError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(())
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
        let store = self.clone();
        let path = self.path();
        let owner = JoinedBlocking::spawn("temper-agent-session-load", move || store.load_sync());
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

    #[allow(dead_code)]
    pub async fn delete(&self) -> Result<bool, AgentSessionStoreError> {
        let store = self.clone();
        let path = self.path();
        JoinedBlocking::spawn("temper-agent-session-delete", move || store.delete_sync())
            .await
            .map_err(|source| AgentSessionStoreError::Io { path, source })?
    }

    fn validate_loaded(
        &self,
        path: &Path,
        stored: &StoredAgentSession,
    ) -> Result<(), AgentSessionStoreError> {
        if stored.version != STORE_VERSION {
            return Err(AgentSessionStoreError::UnsupportedVersion {
                path: path.to_path_buf(),
                version: stored.version,
            });
        }
        if stored.role != self.role || stored.coordination_key != self.coordination_key {
            return Err(AgentSessionStoreError::KeyMismatch {
                path: path.to_path_buf(),
                expected_role: self.role.clone(),
                expected_key: self.coordination_key.clone(),
                found_role: stored.role.clone(),
                found_key: stored.coordination_key.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct StoredAgentSession {
    version: u32,
    role: String,
    coordination_key: String,
    state: AgentSessionState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn save_load_and_delete_session_by_role_and_coordination_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AgentSessionStore::for_workspace_root(temp.path(), "engineer", "pr-for-code-7")
            .expect("store");
        let mut state = AgentSessionState::new("session-1");
        state.state = Some(json!({ "provider": "test" }));

        store.save_sync(&state).expect("save");
        assert_eq!(store.load_sync().expect("load"), Some(state));
        assert!(store.delete_sync().expect("delete"));
        assert_eq!(store.load_sync().expect("load after delete"), None);
        assert!(!store.delete_sync().expect("delete missing"));
    }

    #[test]
    fn session_store_uses_one_safe_path_component_for_coordination_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store =
            AgentSessionStore::for_workspace_root(temp.path(), "engineer", "../../escape/nested")
                .expect("store");

        assert!(store.path().starts_with(temp.path()));
        assert!(store.path().ends_with(
            "engineer/%2E%2E%2F%2E%2E%2Fescape%2Fnested/.temper-agent-session/state.json"
        ));
        assert!(AgentSessionStore::for_workspace_root(temp.path(), "../bad", "key").is_err());
        assert!(AgentSessionStore::for_workspace_root(temp.path(), "engineer", "  ").is_err());
    }

    #[test]
    fn session_store_has_no_cross_key_leakage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let first = AgentSessionStore::for_workspace_root(temp.path(), "engineer", "key-1")
            .expect("first store");
        let second = AgentSessionStore::for_workspace_root(temp.path(), "engineer", "key-2")
            .expect("second store");
        first
            .save_sync(&AgentSessionState::new("session-1"))
            .expect("save first");

        assert_eq!(second.load_sync().expect("load second"), None);

        let second_path = second.path();
        std::fs::create_dir_all(second_path.parent().expect("session parent"))
            .expect("create second parent");
        std::fs::copy(first.path(), &second_path).expect("copy mismatched file");
        let error = second.load_sync().expect_err("mismatch rejected");
        assert!(matches!(error, AgentSessionStoreError::KeyMismatch { .. }));
    }
}
