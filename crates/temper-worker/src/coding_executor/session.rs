use std::path::Path;

use temper_protocol_agent::{AgentSessionState, WorkspaceContext};
use temper_protocol_worker::FailureClass;

use crate::agent_session::AgentSessionStore;
use crate::executor::{JobCancellation, JobOutcome};

use super::{JobMode, failure};

#[derive(Debug)]
pub(super) struct AgentSessionBinding {
    store: AgentSessionStore,
    state: AgentSessionState,
}

pub(super) async fn attach_agent_session(
    context: &mut WorkspaceContext,
    workspace_root: &Path,
    role: &str,
    coordination_key: &str,
    mode: JobMode,
    cancellation: &JobCancellation,
) -> Result<Option<AgentSessionBinding>, JobOutcome> {
    if !session_enabled(role, mode) {
        return Ok(None);
    }

    let store = AgentSessionStore::for_workspace_root(workspace_root, role, coordination_key)
        .map_err(|error| {
            failure(
                FailureClass::Protocol,
                format!("invalid agent session store path: {error}"),
            )
        })?;
    let state = match store.load_controlled(cancellation).await {
        Ok(Some(state)) => {
            tracing::debug!(
                target: "temper_worker",
                role,
                coordination_key,
                session_id = %state.session_id,
                "resuming saved agent session"
            );
            state
        }
        Ok(None) => {
            if mode == JobMode::PullRequestWritable {
                tracing::warn!(
                    target: "temper_worker",
                    role,
                    coordination_key,
                    "no saved agent session for PR feedback; starting a new session"
                );
            } else {
                tracing::debug!(
                    target: "temper_worker",
                    role,
                    coordination_key,
                    "starting a new agent session"
                );
            }
            new_session_state()
        }
        Err(error) => {
            tracing::warn!(
                target: "temper_worker",
                role,
                coordination_key,
                %error,
                "could not load saved agent session; starting a new session"
            );
            new_session_state()
        }
    };

    context.agent_session = Some(state.clone());
    // Persist the identity before the agent is started. A watchdog timeout must
    // preserve this coordination-scoped session even though the ordinary
    // success cleanup path is never reached.
    store
        .save_controlled(&state, cancellation)
        .await
        .map_err(|error| {
            failure(
                FailureClass::Transient,
                format!("save attached agent session state: {error}"),
            )
        })?;
    Ok(Some(AgentSessionBinding { store, state }))
}

pub(super) async fn persist_after_success(
    binding: Option<&AgentSessionBinding>,
    outcome: &JobOutcome,
    cancellation: &JobCancellation,
) -> Option<JobOutcome> {
    if !matches!(outcome, JobOutcome::Success { .. }) {
        return None;
    }
    let binding = binding?;
    match binding
        .store
        .save_controlled(&binding.state, cancellation)
        .await
    {
        Ok(()) => {
            tracing::debug!(
                target: "temper_worker",
                session_id = %binding.state.session_id,
                "saved agent session for paused workstream"
            );
            None
        }
        Err(error) => Some(failure(
            FailureClass::Transient,
            format!("save agent session state: {error}"),
        )),
    }
}

fn session_enabled(role: &str, mode: JobMode) -> bool {
    role == "engineer" && matches!(mode, JobMode::Writable | JobMode::PullRequestWritable)
}

fn new_session_state() -> AgentSessionState {
    AgentSessionState::new(uuid::Uuid::new_v4().to_string())
}
