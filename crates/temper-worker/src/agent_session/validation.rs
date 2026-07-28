// SPDX-License-Identifier: MPL-2.0

//! Complete state-machine validation for durable session-recovery ledgers.

use std::path::Path;

use temper_protocol_activity::ModelFailureDispositionV1;
use temper_protocol_worker::SessionRecoveryActionV1;

use super::{
    AgentSessionLedger, AgentSessionLedgerV2, AgentSessionStoreError, EVIDENCE_LOCATION,
    LEGACY_SESSION_FAILURE_LIMIT,
};

pub(super) fn validate_ledger(
    path: &Path,
    ledger: &AgentSessionLedger,
) -> Result<(), AgentSessionStoreError> {
    let invalid = |reason: &str| AgentSessionStoreError::InvalidLedger {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };
    if ledger.active_session.session_id.trim().is_empty() {
        return Err(invalid("active session id must not be empty"));
    }
    if ledger.failure_epoch == 0 || ledger.current_session_number == 0 {
        return Err(invalid(
            "failure epoch and session number must be greater than zero",
        ));
    }
    ledger.recovery_policy.validate().map_err(invalid)?;
    if ledger.current_session_number != ledger.fresh_sessions_used.saturating_add(1)
        || ledger.fresh_sessions_used > ledger.recovery_policy.fresh_session_limit
    {
        return Err(invalid(
            "session number disagrees with the fresh-session budget",
        ));
    }
    if ledger.session_terminal_failures > ledger.cumulative_terminal_failures {
        return Err(invalid(
            "session terminal failures exceed the cumulative epoch count",
        ));
    }
    if ledger.deferral_count > ledger.recovery_policy.provider_deferral_limit
        || ledger.deferral_generation < ledger.deferral_count
    {
        return Err(invalid("deferral evidence exceeds its configured budget"));
    }

    if ledger.cumulative_terminal_failures == 0 {
        if ledger.session_terminal_failures != 0
            || ledger.failure_epoch_started_unix_ms.is_some()
            || ledger.failure_epoch_elapsed_ms != 0
            || ledger.configured_slo_deadline_unix_ms.is_some()
            || ledger.latest_model_failure.is_some()
            || ledger.immediate_retry_exhausted
            || ledger.deferral_count != 0
            || ledger.deferral_generation != 0
            || ledger.not_before_unix_ms.is_some()
            || ledger.accounted_attempt_id.is_some()
            || ledger.recovery_decision.is_some()
        {
            return Err(invalid(
                "empty failure epoch contains active recovery evidence",
            ));
        }
    } else {
        let started = ledger
            .failure_epoch_started_unix_ms
            .ok_or_else(|| invalid("failure epoch is missing its start time"))?;
        let deadline = ledger
            .configured_slo_deadline_unix_ms
            .ok_or_else(|| invalid("failure epoch is missing its SLO deadline"))?;
        if deadline <= started
            || started.checked_add(ledger.recovery_policy.recovery_slo_ms) != Some(deadline)
        {
            return Err(invalid(
                "configured SLO deadline does not match the snapshotted recovery policy",
            ));
        }
        let diagnostic = ledger
            .latest_model_failure
            .as_ref()
            .ok_or_else(|| invalid("failure epoch is missing canonical model evidence"))?;
        diagnostic
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
        let expected_exhaustion = matches!(
            diagnostic.disposition,
            ModelFailureDispositionV1::Retryable | ModelFailureDispositionV1::Unknown
        );
        if ledger.immediate_retry_exhausted != expected_exhaustion {
            return Err(invalid(
                "immediate retry evidence disagrees with canonical disposition",
            ));
        }
    }

    if let Some(prior) = &ledger.prior_session {
        if prior.session.session_id.trim().is_empty()
            || prior.session.session_id == ledger.active_session.session_id
            || prior.session_number == 0
            || prior.session_terminal_failures == 0
            || prior.cumulative_terminal_failures < prior.session_terminal_failures
            || prior.failed_attempt_id.trim().is_empty()
        {
            return Err(invalid("prior-session record is incomplete"));
        }
        legacy_identity_evidence(
            ledger.failure_epoch,
            &prior.failed_attempt_id,
            &prior.session.session_id,
        )
        .validate_for_attempt(Some(&prior.failed_attempt_id))
        .map_err(|reason| invalid(&format!("invalid prior-session record: {reason}")))?;
        prior
            .model_failure
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
    }

    match (&ledger.accounted_attempt_id, &ledger.recovery_decision) {
        (None, None) => Ok(()),
        (Some(attempt_id), Some(decision)) => {
            decision
                .validate_for_attempt(Some(attempt_id))
                .map_err(|reason| invalid(&reason))?;
            let diagnostic = ledger
                .latest_model_failure
                .as_ref()
                .ok_or_else(|| invalid("accounted decision has no canonical model evidence"))?;
            if decision.failure_epoch != ledger.failure_epoch
                || decision.failure_count != ledger.cumulative_terminal_failures
                || decision.epoch_started_unix_ms != ledger.failure_epoch_started_unix_ms
                || decision.epoch_elapsed_ms != ledger.failure_epoch_elapsed_ms
                || decision.slo_deadline_unix_ms != ledger.configured_slo_deadline_unix_ms
                || decision.disposition != Some(diagnostic.disposition)
                || decision.immediate_retry_exhausted != ledger.immediate_retry_exhausted
                || decision.configured_session_failure_limit
                    != ledger.recovery_policy.session_failure_limit
                || decision.configured_fresh_session_limit
                    != ledger.recovery_policy.fresh_session_limit
                || decision.configured_deferral_limit
                    != ledger.recovery_policy.provider_deferral_limit
                || decision.deferral_count != ledger.deferral_count
                || decision.deferral_generation != ledger.deferral_generation
                || decision.not_before_unix_ms != ledger.not_before_unix_ms
                || decision.evidence_location != EVIDENCE_LOCATION
            {
                return Err(invalid(
                    "recovery decision disagrees with its ledger evidence",
                ));
            }
            validate_action(path, ledger, attempt_id, decision)
        }
        _ => Err(invalid(
            "accounted attempt and recovery decision must be present together",
        )),
    }
}

fn validate_action(
    path: &Path,
    ledger: &AgentSessionLedger,
    attempt_id: &str,
    decision: &temper_protocol_worker::SessionRecoveryEvidenceV1,
) -> Result<(), AgentSessionStoreError> {
    let invalid = |reason: &str| AgentSessionStoreError::InvalidLedger {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };
    let prior_id = ledger
        .prior_session
        .as_ref()
        .map(|prior| prior.session.session_id.as_str());
    match decision.action {
        SessionRecoveryActionV1::RetryCurrentSession => {
            if ledger.active_session.session_id != decision.current_session_id
                || decision.session_number != ledger.current_session_number
                || decision.session_failure_count != ledger.session_terminal_failures
                || decision.prior_session_id.as_deref() != prior_id
                || ledger.session_terminal_failures >= ledger.recovery_policy.session_failure_limit
                || ledger.failure_epoch_elapsed_ms >= ledger.recovery_policy.recovery_slo_ms
                || decision.disposition == Some(ModelFailureDispositionV1::NonRetryable)
                || ledger.not_before_unix_ms.is_some()
            {
                return Err(invalid(
                    "retry decision does not match the active session budget",
                ));
            }
        }
        SessionRecoveryActionV1::RotateSession => {
            let prior = ledger
                .prior_session
                .as_ref()
                .ok_or_else(|| invalid("rotation decision has no retained predecessor"))?;
            if prior.session.session_id != decision.current_session_id
                || prior.failed_attempt_id != attempt_id
                || prior.session_number != decision.session_number
                || prior.session_terminal_failures != decision.session_failure_count
                || prior.cumulative_terminal_failures != decision.failure_count
                || prior.model_failure != *ledger.latest_model_failure.as_ref().unwrap()
                || decision.new_session_id.as_deref()
                    != Some(ledger.active_session.session_id.as_str())
                || ledger.current_session_number != decision.session_number.saturating_add(1)
                || ledger.session_terminal_failures != 0
                || ledger.fresh_sessions_used == 0
                || ledger.failure_epoch_elapsed_ms >= ledger.recovery_policy.recovery_slo_ms
                || ledger.not_before_unix_ms.is_some()
            {
                return Err(invalid(
                    "rotation decision does not match the persisted boundary",
                ));
            }
        }
        SessionRecoveryActionV1::ProviderDeferred => {
            if ledger.active_session.session_id != decision.current_session_id
                || decision.session_number != ledger.current_session_number
                || decision.session_failure_count != ledger.session_terminal_failures
                || decision.prior_session_id.as_deref() != prior_id
                || ledger.fresh_sessions_used < ledger.recovery_policy.fresh_session_limit
                || ledger.deferral_count == 0
                || decision.disposition == Some(ModelFailureDispositionV1::NonRetryable)
                || ledger.failure_epoch_elapsed_ms >= ledger.recovery_policy.recovery_slo_ms
                || ledger.not_before_unix_ms.is_none()
            {
                return Err(invalid(
                    "provider deferral does not match exhausted automatic recovery",
                ));
            }
        }
        SessionRecoveryActionV1::ParkForHuman => {
            let policy_exhausted = ledger.failure_epoch_elapsed_ms
                >= ledger.recovery_policy.recovery_slo_ms
                || (ledger.fresh_sessions_used >= ledger.recovery_policy.fresh_session_limit
                    && ledger.deferral_count >= ledger.recovery_policy.provider_deferral_limit);
            if ledger.active_session.session_id != decision.current_session_id
                || decision.session_number != ledger.current_session_number
                || decision.session_failure_count != ledger.session_terminal_failures
                || decision.prior_session_id.as_deref() != prior_id
                || (decision.disposition != Some(ModelFailureDispositionV1::NonRetryable)
                    && !policy_exhausted)
                || ledger.not_before_unix_ms.is_some()
            {
                return Err(invalid(
                    "human park decision does not match the active failure boundary",
                ));
            }
        }
    }
    Ok(())
}

/// Strict validation of the previous on-disk shape before any migration write.
pub(super) fn validate_v2_ledger(
    path: &Path,
    ledger: &AgentSessionLedgerV2,
) -> Result<(), AgentSessionStoreError> {
    let invalid = |reason: &str| AgentSessionStoreError::InvalidLedger {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };
    if ledger.active_session.session_id.trim().is_empty() || ledger.failure_epoch == 0 {
        return Err(invalid("V2 session identity or failure epoch is invalid"));
    }
    if ledger.consecutive_terminal_count > LEGACY_SESSION_FAILURE_LIMIT {
        return Err(invalid("V2 terminal count exceeds its session budget"));
    }
    if let Some(diagnostic) = &ledger.latest_model_failure {
        diagnostic
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
    }
    if ledger.rotation_consumed && ledger.prior_session.is_none() {
        return Err(invalid("V2 consumed rotation has no predecessor evidence"));
    }
    if let Some(prior) = &ledger.prior_session {
        if prior.session.session_id.trim().is_empty()
            || prior.session.session_id == ledger.active_session.session_id
            || prior.failed_attempt_id.trim().is_empty()
            || prior.consecutive_terminal_count == 0
            || prior.consecutive_terminal_count > LEGACY_SESSION_FAILURE_LIMIT
            || ledger.latest_model_failure.is_none()
        {
            return Err(invalid("V2 prior-session record is incomplete"));
        }
        legacy_identity_evidence(
            ledger.failure_epoch,
            &prior.failed_attempt_id,
            &prior.session.session_id,
        )
        .validate_for_attempt(Some(&prior.failed_attempt_id))
        .map_err(|reason| invalid(&format!("invalid V2 prior-session record: {reason}")))?;
        prior
            .model_failure
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
        if prior.model_failure.disposition == ModelFailureDispositionV1::Retryable
            && prior.consecutive_terminal_count != LEGACY_SESSION_FAILURE_LIMIT
        {
            return Err(invalid(
                "V2 retryable predecessor did not exhaust its session budget",
            ));
        }
        if !ledger.rotation_consumed && ledger.failure_epoch == 1 {
            return Err(invalid(
                "V2 retained predecessor requires a completed failure epoch",
            ));
        }
    }

    match (&ledger.accounted_attempt_id, &ledger.recovery_decision) {
        (None, None) => {
            if ledger.consecutive_terminal_count != 0 {
                return Err(invalid(
                    "V2 nonzero terminal count has no accounted decision",
                ));
            }
            if ledger.rotation_consumed {
                return Err(invalid("V2 consumed rotation has no accounted decision"));
            }
            if ledger.failure_epoch == 1
                && (ledger.latest_model_failure.is_some() || ledger.prior_session.is_some())
            {
                return Err(invalid(
                    "V2 initial epoch contains reset-only recovery history",
                ));
            }
            if ledger.prior_session.is_none()
                && ledger.latest_model_failure.as_ref().is_some_and(|failure| {
                    failure.disposition == ModelFailureDispositionV1::NonRetryable
                })
            {
                return Err(invalid(
                    "V2 unrotated reset retains a known non-retryable failure",
                ));
            }
            Ok(())
        }
        (Some(attempt_id), Some(decision)) => {
            let diagnostic = ledger
                .latest_model_failure
                .as_ref()
                .ok_or_else(|| invalid("V2 accounted decision has no model failure"))?;
            decision
                .validate_for_attempt(Some(attempt_id))
                .map_err(|reason| invalid(&reason))?;
            if decision.failure_epoch != ledger.failure_epoch
                || decision.evidence_location != EVIDENCE_LOCATION
            {
                return Err(invalid("V2 decision disagrees with its ledger"));
            }
            let prior_session_id = ledger
                .prior_session
                .as_ref()
                .map(|prior| prior.session.session_id.as_str());
            let consumes_session = diagnostic.disposition != ModelFailureDispositionV1::Retryable
                || decision.failure_count == LEGACY_SESSION_FAILURE_LIMIT;
            match decision.action {
                SessionRecoveryActionV1::RetryCurrentSession => {
                    if decision.current_session_id != ledger.active_session.session_id
                        || decision.failure_count != ledger.consecutive_terminal_count
                        || decision.prior_session_id.as_deref() != prior_session_id
                        || decision.new_session_id.is_some()
                        || diagnostic.disposition == ModelFailureDispositionV1::NonRetryable
                        || decision.failure_count >= LEGACY_SESSION_FAILURE_LIMIT
                    {
                        return Err(invalid("V2 retry decision is inconsistent"));
                    }
                }
                SessionRecoveryActionV1::RotateSession => {
                    let prior = ledger
                        .prior_session
                        .as_ref()
                        .ok_or_else(|| invalid("V2 rotation has no predecessor"))?;
                    if prior.session.session_id != decision.current_session_id
                        || prior.failed_attempt_id != *attempt_id
                        || prior.consecutive_terminal_count != decision.failure_count
                        || prior.model_failure != *diagnostic
                        || decision.new_session_id.as_deref()
                            != Some(ledger.active_session.session_id.as_str())
                        || ledger.consecutive_terminal_count != 0
                        || !ledger.rotation_consumed
                        || !consumes_session
                    {
                        return Err(invalid("V2 rotation decision is inconsistent"));
                    }
                    if ledger.failure_epoch == 1 && decision.prior_session_id.is_some() {
                        return Err(invalid(
                            "V2 initial rotation cannot name an older predecessor",
                        ));
                    }
                    if decision.prior_session_id.as_deref()
                        == Some(decision.current_session_id.as_str())
                        || decision.prior_session_id.as_deref()
                            == decision.new_session_id.as_deref()
                    {
                        return Err(invalid("V2 rotation contains duplicate session ids"));
                    }
                }
                SessionRecoveryActionV1::ParkForHuman => {
                    if decision.current_session_id != ledger.active_session.session_id
                        || decision.failure_count != ledger.consecutive_terminal_count
                        || decision.prior_session_id.as_deref() != prior_session_id
                        || decision.new_session_id.is_some()
                        || !ledger.rotation_consumed
                        || !consumes_session
                    {
                        return Err(invalid("V2 park decision is inconsistent"));
                    }
                }
                SessionRecoveryActionV1::ProviderDeferred => {
                    return Err(invalid("V2 ledger cannot contain provider deferral"));
                }
            }
            Ok(())
        }
        _ => Err(invalid(
            "V2 accounted attempt and decision must be present together",
        )),
    }
}

fn legacy_identity_evidence(
    failure_epoch: u32,
    attempt_id: &str,
    session_id: &str,
) -> temper_protocol_worker::SessionRecoveryEvidenceV1 {
    temper_protocol_worker::SessionRecoveryEvidenceV1 {
        attempt_id: attempt_id.to_string(),
        failure_epoch,
        failure_count: 1,
        session_number: 0,
        session_failure_count: 0,
        epoch_started_unix_ms: None,
        epoch_elapsed_ms: 0,
        disposition: None,
        immediate_retry_exhausted: false,
        configured_session_failure_limit: 0,
        configured_fresh_session_limit: 0,
        configured_deferral_limit: 0,
        deferral_count: 0,
        deferral_generation: 0,
        not_before_unix_ms: None,
        slo_deadline_unix_ms: None,
        action: SessionRecoveryActionV1::RetryCurrentSession,
        current_session_id: session_id.to_string(),
        prior_session_id: None,
        new_session_id: None,
        evidence_location: EVIDENCE_LOCATION.to_string(),
    }
}
