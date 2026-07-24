// SPDX-License-Identifier: MPL-2.0

//! Bounded evidence for durable agent-session recovery decisions.

use serde::{Deserialize, Serialize};

/// Maximum encoded bytes for assignment and session identities in recovery evidence.
pub const MAX_SESSION_RECOVERY_ID_BYTES: usize = 256;
/// Maximum encoded bytes for the operator-safe location of durable recovery evidence.
pub const MAX_SESSION_RECOVERY_EVIDENCE_LOCATION_BYTES: usize = 1024;

/// The bounded recovery decision made after a terminal model failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRecoveryActionV1 {
    /// Re-run the same durable session within its remaining retry budget.
    RetryCurrentSession,
    /// Archive the failed session and continue once with a fresh session.
    RotateSession,
    /// Stop automatic recovery and require operator attention.
    ParkForHuman,
}

/// Durable, operator-safe evidence for one session recovery decision.
///
/// This DTO deliberately excludes prompts, provider responses, stderr, and a
/// generic text/detail field. Every string is an identity or a location with a
/// strict character set and byte bound.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecoveryEvidenceV1 {
    /// Exact daemon assignment attempt that was accounted for.
    pub attempt_id: String,
    /// One-based consecutive-failure epoch.
    pub failure_epoch: u32,
    /// One-based terminal failure count within the epoch.
    pub failure_count: u32,
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
        }
        safe_location(&self.evidence_location)
    }
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
