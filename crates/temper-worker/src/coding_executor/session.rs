use std::path::Path;

use temper_protocol_activity::ModelFailureV1;
use temper_protocol_agent::{AgentSessionState, WorkspaceContext};
use temper_protocol_worker::{FailureClass, SessionRecoveryActionV1, SessionRecoveryEvidenceV1};

use crate::agent_runner::AgentRunError;
use crate::agent_session::{AgentSessionLedger, AgentSessionStore};
use crate::executor::{AttemptFence, JobCancellation, JobOutcome};

use super::{
    JobMode, cancelled_attempt, failure,
    failure::{failure_with_evidence, failure_with_recovery},
};

#[derive(Debug)]
pub(super) struct AgentSessionBinding {
    store: AgentSessionStore,
    ledger: AgentSessionLedger,
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
    let ledger = match store.load_ledger_controlled(cancellation).await {
        Ok(Some(ledger)) => {
            tracing::debug!(
                target: "temper_worker",
                role,
                coordination_key,
                session_id = %ledger.active_session.session_id,
                failure_epoch = ledger.failure_epoch,
                failure_count = ledger.consecutive_terminal_count,
                rotation_consumed = ledger.rotation_consumed,
                "resuming saved agent session ledger"
            );
            ledger
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
            let ledger = AgentSessionLedger::new(new_session_state());
            // Persist the identity before the agent starts. A watchdog timeout
            // must preserve the same coordination-scoped session.
            store
                .save_ledger_controlled(&ledger, cancellation)
                .await
                .map_err(|error| {
                    failure(
                        FailureClass::Transient,
                        format!("save initial agent session ledger: {error}"),
                    )
                })?;
            ledger
        }
        Err(error) => {
            tracing::error!(
                target: "temper_worker",
                role,
                coordination_key,
                %error,
                "saved agent session ledger is unusable; refusing to overwrite it"
            );
            return Err(failure(
                FailureClass::Protocol,
                format!("load durable agent session ledger (state preserved): {error}"),
            ));
        }
    };

    context.agent_session = Some(ledger.active_session.clone());
    Ok(Some(AgentSessionBinding { store, ledger }))
}

/// Replays a durable decision before invoking an agent for the same daemon
/// attempt. This is the executor-side half of attempt idempotency.
pub(super) fn replay_accounted_attempt(
    binding: Option<&AgentSessionBinding>,
    attempt_id: &str,
) -> Option<JobOutcome> {
    let binding = binding?;
    if binding.ledger.accounted_attempt_id.as_deref() != Some(attempt_id) {
        return None;
    }
    let decision = binding.ledger.recovery_decision.clone()?;
    let diagnostic = binding.ledger.latest_model_failure.clone()?;
    tracing::info!(
        target: "temper_worker",
        attempt_id,
        session_id = %decision.current_session_id,
        failure_epoch = decision.failure_epoch,
        failure_count = decision.failure_count,
        action = ?decision.action,
        "replaying persisted agent session recovery decision"
    );
    Some(recovery_outcome(diagnostic, decision))
}

pub(super) async fn agent_failure_outcome(
    binding: Option<&AgentSessionBinding>,
    attempt_id: &str,
    error: AgentRunError,
    fence: &AttemptFence,
    cancellation: &JobCancellation,
) -> JobOutcome {
    let AgentRunError {
        class,
        message,
        model_failure,
    } = error;
    if model_failure.is_some() && (!fence.is_open() || cancellation.is_cancelled()) {
        return cancelled_attempt();
    }
    if let (Some(binding), Some(model_failure)) = (binding, model_failure.as_ref()) {
        return account_terminal_model_failure(
            binding,
            attempt_id,
            model_failure.clone(),
            cancellation,
        )
        .await;
    }
    failure_with_evidence(class, message, model_failure)
}

/// Accounts an authoritative terminal model diagnostic under the exact
/// worker-owned assignment attempt before publishing the returned outcome.
pub(super) async fn account_terminal_model_failure(
    binding: &AgentSessionBinding,
    attempt_id: &str,
    mut diagnostic: ModelFailureV1,
    cancellation: &JobCancellation,
) -> JobOutcome {
    diagnostic.normalize();
    match binding
        .store
        .account_model_failure_controlled(attempt_id, &diagnostic, cancellation)
        .await
    {
        Ok(decision) => {
            tracing::info!(
                target: "temper_worker",
                attempt_id,
                session_id = %decision.current_session_id,
                new_session_id = decision.new_session_id.as_deref(),
                prior_session_id = decision.prior_session_id.as_deref(),
                failure_epoch = decision.failure_epoch,
                failure_count = decision.failure_count,
                action = ?decision.action,
                model_category = diagnostic.category.as_str(),
                model_retryable = diagnostic.retryable,
                "persisted bounded agent session recovery decision"
            );
            recovery_outcome(diagnostic, decision)
        }
        Err(error) => failure_with_recovery(
            FailureClass::Permanent,
            format!(
                "persist terminal model recovery boundary before retry (automatic requeue disabled): {error}"
            ),
            Some(diagnostic),
            None,
        ),
    }
}

pub(super) async fn persist_after_success(
    binding: Option<&AgentSessionBinding>,
    outcome: &JobOutcome,
    cancellation: &JobCancellation,
) -> Option<JobOutcome> {
    if !matches!(
        outcome,
        JobOutcome::Success { .. } | JobOutcome::Verdict { .. }
    ) {
        return None;
    }
    let binding = binding?;
    match binding
        .store
        .reset_after_success_controlled(cancellation)
        .await
    {
        Ok(ledger) => {
            tracing::debug!(
                target: "temper_worker",
                session_id = %ledger.active_session.session_id,
                failure_epoch = ledger.failure_epoch,
                "reset agent session failure epoch after authoritative success"
            );
            None
        }
        Err(error) => Some(failure(
            FailureClass::Permanent,
            format!("persist successful agent session recovery boundary: {error}"),
        )),
    }
}

fn recovery_outcome(diagnostic: ModelFailureV1, decision: SessionRecoveryEvidenceV1) -> JobOutcome {
    let (class, message) = match decision.action {
        SessionRecoveryActionV1::RetryCurrentSession => (
            FailureClass::Transient,
            format!(
                "terminal model failure `{}` on session `{}` (run {} of {}); retrying the same session; durable evidence: {}",
                diagnostic.category.as_str(),
                decision.current_session_id,
                decision.failure_count,
                RETRYABLE_RUN_LIMIT,
                decision.evidence_location,
            ),
        ),
        SessionRecoveryActionV1::RotateSession => {
            let new_session = decision
                .new_session_id
                .as_deref()
                .expect("validated rotation decision has a new session");
            (
                FailureClass::Transient,
                format!(
                    "terminal model failure `{}` consumed session `{}`; rotated from `{}` to `{}` over the preserved workspace; durable evidence: {}",
                    diagnostic.category.as_str(),
                    decision.current_session_id,
                    decision.current_session_id,
                    new_session,
                    decision.evidence_location,
                ),
            )
        }
        SessionRecoveryActionV1::ParkForHuman => (
            FailureClass::Permanent,
            format!(
                "terminal model failure `{}` exhausted bounded recovery on session `{}` after {} run(s); prior session: {}; workspace and durable evidence preserved at {}",
                diagnostic.category.as_str(),
                decision.current_session_id,
                decision.failure_count,
                decision.prior_session_id.as_deref().unwrap_or("none"),
                decision.evidence_location,
            ),
        ),
    };
    failure_with_recovery(class, message, Some(diagnostic), Some(decision))
}

const RETRYABLE_RUN_LIMIT: u32 = 3;

fn session_enabled(role: &str, mode: JobMode) -> bool {
    role == "engineer" && matches!(mode, JobMode::Writable | JobMode::PullRequestWritable)
}

fn new_session_state() -> AgentSessionState {
    AgentSessionState::new(uuid::Uuid::new_v4().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_accounting_failure_is_permanent_and_does_not_requeue_consumed_session() {
        temper_worker_io::block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let store = AgentSessionStore::for_workspace_root(
                temp.path(),
                "engineer",
                "atomic-save-failure",
            )
            .unwrap();
            store
                .save_sync(&AgentSessionState::new("session-old"))
                .unwrap();
            let ledger = store.load_ledger_sync().unwrap().unwrap();
            let binding = AgentSessionBinding {
                store: store.clone().with_replace_failure(),
                ledger,
            };

            let outcome = account_terminal_model_failure(
                &binding,
                "attempt-atomic-failure",
                ModelFailureV1::redacted_unknown("provider", "model", false),
                &JobCancellation::default(),
            )
            .await;
            match outcome {
                JobOutcome::Failure {
                    class,
                    model_failure: Some(_),
                    session_recovery: None,
                    message,
                } => {
                    assert_eq!(class, FailureClass::Permanent);
                    assert!(message.contains("automatic requeue disabled"));
                }
                other => panic!("expected fail-closed permanent outcome, got {other:?}"),
            }
            let unchanged = store.load_ledger_sync().unwrap().unwrap();
            assert_eq!(unchanged.active_session.session_id, "session-old");
            assert_eq!(unchanged.consecutive_terminal_count, 0);
            assert!(!unchanged.rotation_consumed);
        });
    }

    #[test]
    fn verdict_reset_failure_is_permanent_and_preserves_the_old_epoch() {
        temper_worker_io::block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let store = AgentSessionStore::for_workspace_root(
                temp.path(),
                "engineer",
                "verdict-reset-failure",
            )
            .unwrap();
            store
                .save_sync(&AgentSessionState::new("session-old"))
                .unwrap();
            let ledger = store.load_ledger_sync().unwrap().unwrap();
            let binding = AgentSessionBinding {
                store: store.clone().with_replace_failure(),
                ledger,
            };
            let verdict = JobOutcome::Verdict {
                verdict: "needs_architect".to_string(),
                title: None,
                body: Some("blocked".to_string()),
                summary: None,
                children: Vec::new(),
            };

            let outcome =
                persist_after_success(Some(&binding), &verdict, &JobCancellation::default())
                    .await
                    .expect("failed verdict reset must replace the verdict");
            match outcome {
                JobOutcome::Failure { class, message, .. } => {
                    assert_eq!(class, FailureClass::Permanent);
                    assert!(message.contains("persist successful agent session recovery boundary"));
                }
                other => panic!("expected fail-closed permanent outcome, got {other:?}"),
            }

            let unchanged = store.load_ledger_sync().unwrap().unwrap();
            assert_eq!(unchanged.failure_epoch, 1);
            assert_eq!(unchanged.consecutive_terminal_count, 0);
        });
    }
}
