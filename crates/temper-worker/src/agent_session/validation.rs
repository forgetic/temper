// SPDX-License-Identifier: MPL-2.0

//! Complete state-machine validation for durable V2 session-recovery ledgers.

use std::path::Path;

use temper_protocol_worker::{SessionRecoveryActionV1, SessionRecoveryEvidenceV1};

use super::{
    AgentSessionLedger, AgentSessionStoreError, EVIDENCE_LOCATION, INITIAL_FAILURE_EPOCH,
    RETRYABLE_SESSION_FAILURE_LIMIT,
};

pub(super) fn validate_ledger(
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
    if ledger.consecutive_terminal_count > RETRYABLE_SESSION_FAILURE_LIMIT {
        return Err(invalid(format!(
            "consecutive terminal count must not exceed {RETRYABLE_SESSION_FAILURE_LIMIT}"
        )));
    }
    if let Some(diagnostic) = &ledger.latest_model_failure {
        diagnostic
            .validate()
            .map_err(|error| invalid(error.to_string()))?;
    }
    if let Some(prior) = &ledger.prior_session {
        if prior.consecutive_terminal_count == 0
            || prior.consecutive_terminal_count > RETRYABLE_SESSION_FAILURE_LIMIT
        {
            return Err(invalid("prior-session record is incomplete".to_string()));
        }
        SessionRecoveryEvidenceV1 {
            attempt_id: prior.failed_attempt_id.clone(),
            failure_epoch: ledger.failure_epoch,
            failure_count: prior.consecutive_terminal_count,
            action: SessionRecoveryActionV1::RetryCurrentSession,
            current_session_id: prior.session.session_id.clone(),
            prior_session_id: None,
            new_session_id: None,
            evidence_location: EVIDENCE_LOCATION.to_string(),
        }
        .validate_for_attempt(Some(&prior.failed_attempt_id))
        .map_err(|reason| invalid(format!("invalid prior-session record: {reason}")))?;
        if prior.session.session_id == ledger.active_session.session_id {
            return Err(invalid(
                "active and prior session ids must be distinct".to_string(),
            ));
        }
        prior
            .model_failure
            .validate()
            .map_err(|error| invalid(error.to_string()))?;
        if prior.model_failure.retryable
            && prior.consecutive_terminal_count != RETRYABLE_SESSION_FAILURE_LIMIT
        {
            return Err(invalid(
                "a retryable prior-session failure must exhaust the session run limit".to_string(),
            ));
        }
        if ledger.latest_model_failure.is_none() {
            return Err(invalid(
                "a prior-session record requires latest model-failure evidence".to_string(),
            ));
        }
        if !ledger.rotation_consumed && ledger.failure_epoch == INITIAL_FAILURE_EPOCH {
            return Err(invalid(
                "a retained prior session requires a completed failure epoch".to_string(),
            ));
        }
    }
    if ledger.rotation_consumed && ledger.prior_session.is_none() {
        return Err(invalid(
            "a consumed rotation requires a prior-session record".to_string(),
        ));
    }

    match (&ledger.accounted_attempt_id, &ledger.recovery_decision) {
        (None, None) => {
            if ledger.consecutive_terminal_count != 0 {
                return Err(invalid(
                    "a nonzero terminal count requires an accounted recovery decision".to_string(),
                ));
            }
            if ledger.rotation_consumed {
                return Err(invalid(
                    "a consumed rotation requires an accounted recovery decision".to_string(),
                ));
            }
            if ledger.failure_epoch == INITIAL_FAILURE_EPOCH
                && (ledger.latest_model_failure.is_some() || ledger.prior_session.is_some())
            {
                return Err(invalid(
                    "initial failure epoch cannot contain reset-only recovery history".to_string(),
                ));
            }
            if ledger.prior_session.is_none()
                && ledger
                    .latest_model_failure
                    .as_ref()
                    .is_some_and(|diagnostic| !diagnostic.retryable)
            {
                return Err(invalid(
                    "an unrotated success boundary cannot retain a non-retryable failure"
                        .to_string(),
                ));
            }
        }
        (Some(attempt_id), Some(decision)) => {
            let diagnostic = ledger.latest_model_failure.as_ref().ok_or_else(|| {
                invalid(
                    "an accounted recovery decision requires latest model-failure evidence"
                        .to_string(),
                )
            })?;
            decision
                .validate_for_attempt(Some(attempt_id))
                .map_err(invalid)?;
            if decision.failure_epoch != ledger.failure_epoch {
                return Err(invalid(
                    "recovery decision failure epoch does not match the ledger".to_string(),
                ));
            }
            if decision.evidence_location != EVIDENCE_LOCATION {
                return Err(invalid(
                    "recovery decision evidence location does not match the session ledger"
                        .to_string(),
                ));
            }

            let prior_session_id = ledger
                .prior_session
                .as_ref()
                .map(|prior| prior.session.session_id.as_str());
            let consumes_session =
                !diagnostic.retryable || decision.failure_count == RETRYABLE_SESSION_FAILURE_LIMIT;
            match decision.action {
                SessionRecoveryActionV1::RotateSession => {
                    let prior = ledger.prior_session.as_ref().ok_or_else(|| {
                        invalid("rotation decision has no prior-session record".to_string())
                    })?;
                    if prior.session.session_id != decision.current_session_id
                        || decision.new_session_id.as_deref()
                            != Some(ledger.active_session.session_id.as_str())
                        || prior.failed_attempt_id != attempt_id.as_str()
                        || prior.consecutive_terminal_count != decision.failure_count
                        || prior.model_failure != *diagnostic
                        || ledger.consecutive_terminal_count != 0
                        || !ledger.rotation_consumed
                        || !consumes_session
                    {
                        return Err(invalid(
                            "rotation decision does not match the persisted failure boundary"
                                .to_string(),
                        ));
                    }
                    if ledger.failure_epoch == INITIAL_FAILURE_EPOCH
                        && decision.prior_session_id.is_some()
                    {
                        return Err(invalid(
                            "the initial epoch rotation cannot have an older predecessor"
                                .to_string(),
                        ));
                    }
                    if decision.prior_session_id.as_deref()
                        == Some(decision.current_session_id.as_str())
                        || decision.prior_session_id.as_deref()
                            == decision.new_session_id.as_deref()
                    {
                        return Err(invalid(
                            "rotation session boundary contains duplicate session ids".to_string(),
                        ));
                    }
                }
                SessionRecoveryActionV1::RetryCurrentSession => {
                    if ledger.active_session.session_id != decision.current_session_id
                        || ledger.consecutive_terminal_count != decision.failure_count
                        || decision.prior_session_id.as_deref() != prior_session_id
                        || decision.new_session_id.is_some()
                        || !diagnostic.retryable
                        || decision.failure_count >= RETRYABLE_SESSION_FAILURE_LIMIT
                    {
                        return Err(invalid(
                            "retry decision does not match the active session retry budget"
                                .to_string(),
                        ));
                    }
                }
                SessionRecoveryActionV1::ParkForHuman => {
                    if ledger.active_session.session_id != decision.current_session_id
                        || ledger.consecutive_terminal_count != decision.failure_count
                        || decision.prior_session_id.as_deref() != prior_session_id
                        || decision.new_session_id.is_some()
                        || !ledger.rotation_consumed
                        || !consumes_session
                    {
                        return Err(invalid(
                            "park decision does not match the exhausted session boundary"
                                .to_string(),
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
