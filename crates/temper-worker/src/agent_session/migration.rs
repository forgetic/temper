// SPDX-License-Identifier: MPL-2.0

//! Strict previous-version DTOs and the one-way V2→V3 projection.

use std::path::Path;

use serde::{Deserialize, Serialize};
use temper_protocol_activity::{ModelFailureDispositionV1, ModelFailureV1};
use temper_protocol_agent::AgentSessionState;
use temper_protocol_worker::{SessionRecoveryActionV1, SessionRecoveryEvidenceV1};

use super::{
    AgentSessionLedger, AgentSessionStoreError, PriorAgentSessionRecord, SessionRecoveryPolicy,
    invalid, system_time_unix_ms, validation,
};

pub(super) fn migrate_v2_ledger(
    path: &Path,
    old: AgentSessionLedgerV2,
    mut policy: SessionRecoveryPolicy,
) -> Result<AgentSessionLedger, AgentSessionStoreError> {
    policy.validate().map_err(|reason| invalid(path, reason))?;
    // A valid previous ledger may already contain history beyond a newly
    // tightened runtime policy. Preserve that history; the configured policy is
    // selected afresh only after authoritative success starts a new epoch.
    if old.rotation_consumed {
        policy.fresh_session_limit = policy.fresh_session_limit.max(1);
    }
    if old
        .recovery_decision
        .as_ref()
        .is_some_and(|decision| decision.action == SessionRecoveryActionV1::RetryCurrentSession)
    {
        policy.session_failure_limit = policy
            .session_failure_limit
            .max(old.consecutive_terminal_count.saturating_add(1));
    }
    let now = system_time_unix_ms()?;
    let prior_count = if old.rotation_consumed {
        old.prior_session
            .as_ref()
            .map_or(0, |prior| prior.consecutive_terminal_count)
    } else {
        0
    };
    let has_failure = old.recovery_decision.is_some();
    let cumulative = if has_failure {
        prior_count
            .checked_add(old.consecutive_terminal_count)
            .ok_or_else(|| invalid(path, "migrated cumulative terminal count overflow"))?
    } else {
        0
    };
    let started = has_failure.then_some(now);
    let deadline = if has_failure {
        Some(
            now.checked_add(policy.recovery_slo_ms)
                .ok_or_else(|| invalid(path, "migrated SLO deadline overflow"))?,
        )
    } else {
        None
    };
    let current_session_number: u32 = if old.rotation_consumed { 2 } else { 1 };
    let latest_disposition = old
        .latest_model_failure
        .as_ref()
        .map(|value| value.disposition);
    let immediate_retry_exhausted = has_failure
        && latest_disposition.is_some_and(|value| {
            matches!(
                value,
                ModelFailureDispositionV1::Retryable | ModelFailureDispositionV1::Unknown
            )
        });
    let mut deferral_count = 0;
    let mut deferral_generation = 0;
    let mut not_before = None;
    let recovery_decision = old.recovery_decision.map(|mut decision| {
        let session_failure_count = decision.failure_count;
        decision.failure_count = cumulative.max(1);
        decision.session_number = if decision.action == SessionRecoveryActionV1::RotateSession {
            current_session_number.saturating_sub(1)
        } else {
            current_session_number
        };
        decision.session_failure_count = session_failure_count;
        decision.epoch_started_unix_ms = started;
        decision.epoch_elapsed_ms = 0;
        decision.disposition = latest_disposition;
        decision.immediate_retry_exhausted = immediate_retry_exhausted;
        decision.configured_session_failure_limit = policy.session_failure_limit;
        decision.configured_fresh_session_limit = policy.fresh_session_limit;
        decision.configured_deferral_limit = policy.provider_deferral_limit;
        if decision.action == SessionRecoveryActionV1::ParkForHuman
            && latest_disposition.is_some_and(|value| {
                matches!(
                    value,
                    ModelFailureDispositionV1::Retryable | ModelFailureDispositionV1::Unknown
                )
            })
        {
            decision.action = SessionRecoveryActionV1::ProviderDeferred;
            deferral_count = 1;
            deferral_generation = 1;
            not_before = Some(
                now.saturating_add(policy.provider_deferral_delay_ms)
                    .min(deadline.unwrap_or(now)),
            );
        }
        decision.deferral_count = deferral_count;
        decision.deferral_generation = deferral_generation;
        decision.not_before_unix_ms = not_before;
        decision.slo_deadline_unix_ms = deadline;
        decision
    });
    let ledger = AgentSessionLedger {
        active_session: old.active_session,
        prior_session: old.prior_session.map(|prior| PriorAgentSessionRecord {
            session: prior.session,
            session_number: 1,
            failed_attempt_id: prior.failed_attempt_id,
            session_terminal_failures: prior.consecutive_terminal_count,
            cumulative_terminal_failures: prior.consecutive_terminal_count,
            model_failure: prior.model_failure,
        }),
        failure_epoch: old.failure_epoch,
        cumulative_terminal_failures: cumulative,
        current_session_number,
        session_terminal_failures: old.consecutive_terminal_count,
        fresh_sessions_used: u32::from(old.rotation_consumed),
        failure_epoch_started_unix_ms: started,
        failure_epoch_elapsed_ms: 0,
        configured_slo_deadline_unix_ms: deadline,
        recovery_policy: policy,
        latest_model_failure: if has_failure {
            old.latest_model_failure
        } else {
            None
        },
        immediate_retry_exhausted,
        deferral_count,
        deferral_generation,
        not_before_unix_ms: not_before,
        accounted_attempt_id: old.accounted_attempt_id,
        recovery_decision,
    };
    validation::validate_ledger(path, &ledger)?;
    Ok(ledger)
}

#[derive(Deserialize)]
pub(super) struct StoredVersion {
    pub(super) version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredAgentSessionV1 {
    #[serde(rename = "version")]
    _version: u32,
    pub(super) role: String,
    pub(super) coordination_key: String,
    pub(super) state: AgentSessionState,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PriorAgentSessionRecordV2 {
    pub(super) session: AgentSessionState,
    pub(super) failed_attempt_id: String,
    pub(super) consecutive_terminal_count: u32,
    pub(super) model_failure: ModelFailureV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentSessionLedgerV2 {
    pub(super) active_session: AgentSessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) prior_session: Option<PriorAgentSessionRecordV2>,
    pub(super) failure_epoch: u32,
    pub(super) consecutive_terminal_count: u32,
    pub(super) rotation_consumed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) latest_model_failure: Option<ModelFailureV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) accounted_attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) recovery_decision: Option<SessionRecoveryEvidenceV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredAgentSessionV2 {
    #[serde(rename = "version")]
    _version: u32,
    pub(super) role: String,
    pub(super) coordination_key: String,
    pub(super) ledger: AgentSessionLedgerV2,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredAgentSessionV3 {
    pub(super) version: u32,
    pub(super) role: String,
    pub(super) coordination_key: String,
    pub(super) ledger: AgentSessionLedger,
}
