// SPDX-License-Identifier: MPL-2.0

//! Bounded evidence for durable agent-session recovery decisions.

use serde::{Deserialize, Serialize};
use temper_protocol_activity::ModelFailureDispositionV1;

/// Maximum encoded bytes for assignment and session identities in recovery evidence.
pub const MAX_SESSION_RECOVERY_ID_BYTES: usize = 256;
/// Maximum encoded bytes for the operator-safe location of durable recovery evidence.
pub const MAX_SESSION_RECOVERY_EVIDENCE_LOCATION_BYTES: usize = 1024;

/// The bounded recovery decision made after a terminal model failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRecoveryActionV1 {
    /// Re-run the same durable session within its remaining terminal-run budget.
    RetryCurrentSession,
    /// Archive the failed session and continue with a fresh session.
    RotateSession,
    /// Release the assignment until provider recovery is due, without human parking.
    ProviderDeferred,
    /// Stop automatic recovery and require operator attention.
    ParkForHuman,
}

impl SessionRecoveryActionV1 {
    /// Stable wire/log spelling shared by recovery results and projections.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RetryCurrentSession => "retry_current_session",
            Self::RotateSession => "rotate_session",
            Self::ProviderDeferred => "provider_deferred",
            Self::ParkForHuman => "park_for_human",
        }
    }
}

/// Durable, operator-safe evidence for one session recovery decision.
///
/// This DTO deliberately excludes prompts, provider responses, stderr, and a
/// generic text/detail field. Every string is an identity or a location with a
/// strict character set and byte bound. Fields added after the original V1
/// shape have compatibility defaults so old worker results remain readable;
/// new ledgers always populate the complete extended evidence set.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecoveryEvidenceV1 {
    /// Exact daemon assignment attempt that was accounted for.
    pub attempt_id: String,
    /// One-based consecutive-failure epoch.
    pub failure_epoch: u32,
    /// One-based cumulative terminal failure count for the unsucceeded epoch.
    pub failure_count: u32,
    /// One-based session number within this failure epoch.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub session_number: u32,
    /// One-based terminal failure count in `current_session_id`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub session_failure_count: u32,
    /// Unix time at which the current unsucceeded failure epoch began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch_started_unix_ms: Option<u64>,
    /// Elapsed wall-clock evidence at this decision.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub epoch_elapsed_ms: u64,
    /// Canonical typed recovery authority copied from the diagnostic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<ModelFailureDispositionV1>,
    /// Whether same-turn model-request recovery was exhausted before this boundary.
    #[serde(default, skip_serializing_if = "is_false")]
    pub immediate_retry_exhausted: bool,
    /// Configured terminal-run budget for one session.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub configured_session_failure_limit: u32,
    /// Configured number of fresh sessions allowed in one failure epoch.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub configured_fresh_session_limit: u32,
    /// Configured provider-deferral generations allowed before human parking.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub configured_deferral_limit: u32,
    /// Number of provider deferrals issued in this failure epoch.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub deferral_count: u32,
    /// Monotonic generation used to fence a deferred wake.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub deferral_generation: u32,
    /// Earliest automatic provider-recovery wake for a deferred decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before_unix_ms: Option<u64>,
    /// Absolute configured SLO boundary for this failure epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slo_deadline_unix_ms: Option<u64>,
    pub action: SessionRecoveryActionV1,
    /// Session that produced the terminal model failure.
    pub current_session_id: String,
    /// Archived predecessor, when the current session was created by rotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_session_id: Option<String>,
    /// Fresh session selected by this decision, when rotating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_session_id: Option<String>,
    /// Worker-generated path or URI-like location containing durable evidence.
    pub evidence_location: String,
}

impl SessionRecoveryEvidenceV1 {
    /// Validates bounded shape and, when supplied, the trusted outer attempt.
    pub fn validate_for_attempt(&self, expected_attempt_id: Option<&str>) -> Result<(), String> {
        bounded_attempt_id(&self.attempt_id)?;
        if expected_attempt_id != Some(self.attempt_id.as_str()) {
            return Err("attempt_id does not match the enclosing result".to_string());
        }
        if self.failure_epoch == 0 {
            return Err("failure_epoch must be greater than zero".to_string());
        }
        if self.failure_count == 0 {
            return Err("failure_count must be greater than zero".to_string());
        }
        safe_identifier(&self.current_session_id, "current_session_id")?;
        if let Some(value) = &self.prior_session_id {
            safe_identifier(value, "prior_session_id")?;
        }
        if let Some(value) = &self.new_session_id {
            safe_identifier(value, "new_session_id")?;
        }
        if self.action == SessionRecoveryActionV1::RotateSession {
            let new_session = self
                .new_session_id
                .as_deref()
                .ok_or_else(|| "rotate_session requires new_session_id".to_string())?;
            if new_session == self.current_session_id {
                return Err("rotate_session requires a distinct new_session_id".to_string());
            }
        } else if self.new_session_id.is_some() {
            return Err("only rotate_session may carry new_session_id".to_string());
        }
        self.validate_extended_evidence()?;
        safe_location(&self.evidence_location)
    }

    fn validate_extended_evidence(&self) -> Result<(), String> {
        let extended = self.session_number != 0
            || self.session_failure_count != 0
            || self.epoch_started_unix_ms.is_some()
            || self.epoch_elapsed_ms != 0
            || self.disposition.is_some()
            || self.immediate_retry_exhausted
            || self.configured_session_failure_limit != 0
            || self.configured_fresh_session_limit != 0
            || self.configured_deferral_limit != 0
            || self.deferral_count != 0
            || self.deferral_generation != 0
            || self.not_before_unix_ms.is_some()
            || self.slo_deadline_unix_ms.is_some();
        if !extended {
            // Original V1 results are intentionally still accepted.
            return Ok(());
        }
        if self.session_number == 0 || self.session_failure_count == 0 {
            return Err("extended recovery evidence requires positive session counts".to_string());
        }
        if self.session_failure_count > self.failure_count {
            return Err("session_failure_count must not exceed failure_count".to_string());
        }
        if self.configured_session_failure_limit == 0 || self.configured_deferral_limit == 0 {
            return Err(
                "extended recovery evidence requires configured recovery limits".to_string(),
            );
        }
        if self.deferral_count > self.configured_deferral_limit
            || self.deferral_generation < self.deferral_count
        {
            return Err("deferral evidence exceeds its configured budget".to_string());
        }
        let started = self.epoch_started_unix_ms.ok_or_else(|| {
            "extended recovery evidence requires epoch_started_unix_ms".to_string()
        })?;
        let deadline = self.slo_deadline_unix_ms.ok_or_else(|| {
            "extended recovery evidence requires slo_deadline_unix_ms".to_string()
        })?;
        if deadline <= started {
            return Err("failure epoch SLO deadline must follow its start".to_string());
        }
        let disposition = self
            .disposition
            .ok_or_else(|| "extended recovery evidence requires disposition".to_string())?;
        if self.immediate_retry_exhausted
            != matches!(
                disposition,
                ModelFailureDispositionV1::Retryable | ModelFailureDispositionV1::Unknown
            )
        {
            return Err("immediate retry exhaustion disagrees with disposition".to_string());
        }
        if self.action == SessionRecoveryActionV1::ProviderDeferred {
            if !self.immediate_retry_exhausted {
                return Err("provider_deferred requires exhausted immediate retries".to_string());
            }
            if self.deferral_count == 0 || self.deferral_generation == 0 {
                return Err("provider_deferred requires a positive deferral generation".to_string());
            }
            let not_before = self
                .not_before_unix_ms
                .ok_or_else(|| "provider_deferred requires not_before_unix_ms".to_string())?;
            if not_before <= started || not_before > deadline {
                return Err(
                    "provider_deferred not-before must be within the SLO window".to_string()
                );
            }
        } else if self.not_before_unix_ms.is_some() {
            return Err("only provider_deferred may carry not_before_unix_ms".to_string());
        }
        Ok(())
    }
}

const fn is_zero(value: &u32) -> bool {
    *value == 0
}

const fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

const fn is_false(value: &bool) -> bool {
    !*value
}

fn bounded_attempt_id(value: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.len() > MAX_SESSION_RECOVERY_ID_BYTES
        || value.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                )
        })
    {
        return Err(format!(
            "attempt_id must be 1..={MAX_SESSION_RECOVERY_ID_BYTES} safe UTF-8 bytes"
        ));
    }
    Ok(())
}

fn safe_identifier(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_SESSION_RECOVERY_ID_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err(format!(
            "{field} must be 1..={MAX_SESSION_RECOVERY_ID_BYTES} ASCII bytes using only identifier characters"
        ));
    }
    Ok(())
}

fn safe_location(value: &str) -> Result<(), String> {
    let folded = value.to_ascii_lowercase();
    let sensitive = [
        "authorization",
        "bearer",
        "credential",
        "api_key",
        "access_token",
        "refresh_token",
        "secret",
    ]
    .iter()
    .any(|marker| folded.contains(marker));
    if value.is_empty()
        || value.len() > MAX_SESSION_RECOVERY_EVIDENCE_LOCATION_BYTES
        || sensitive
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'\\')
        })
    {
        return Err(format!(
            "evidence_location must be 1..={MAX_SESSION_RECOVERY_EVIDENCE_LOCATION_BYTES} ASCII bytes using only path/location characters"
        ));
    }
    Ok(())
}
