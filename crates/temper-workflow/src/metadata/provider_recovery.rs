// SPDX-License-Identifier: MPL-2.0

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum durable workstream/attempt/provider identity size.
pub const MAX_PROVIDER_RECOVERY_ID_BYTES: usize = 256;
/// Maximum cumulative counters accepted in durable provider recovery metadata.
pub const MAX_PROVIDER_RECOVERY_COUNT: u32 = 1_000_000;

/// A provider failure that is eligible for bounded automatic recovery.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRecoveryDisposition {
    Retryable,
    Unknown,
}

/// Allowlisted provider facts copied from the canonical model diagnostic.
///
/// Free-form provider messages and response bodies are deliberately absent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRecoveryFacts {
    pub provider: String,
    pub model: String,
    pub category: String,
    pub boundary: String,
    pub event_kind: String,
    /// Whether a typed HTTP status was present before safe normalization.
    pub status_present: bool,
    /// Whether a provider code was present before non-allowlisted text was dropped.
    pub code_present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_error_code: Option<String>,
}

impl ProviderRecoveryFacts {
    pub fn validate(&self) -> Result<(), String> {
        safe_ascii_token(&self.provider, "provider", 128)?;
        safe_ascii_token(&self.model, "model", 256)?;
        allowed(
            &self.category,
            "category",
            &[
                "timeout",
                "transport",
                "rate_limit",
                "authentication",
                "context",
                "response",
                "provider",
                "redacted_unknown",
            ],
        )?;
        allowed(&self.boundary, "boundary", &["http", "sse", "local"])?;
        allowed(
            &self.event_kind,
            "event_kind",
            &[
                "http_response",
                "stream_error",
                "error_completion",
                "stream_eof",
                "connect_timeout",
                "stream_idle_timeout",
                "transport",
                "local_error",
            ],
        )?;
        if self
            .http_status
            .is_some_and(|status| !(100..=599).contains(&status))
        {
            return Err("http_status must be a valid HTTP status".to_string());
        }
        if self.http_status.is_some() && !self.status_present {
            return Err("status_present must be true when http_status is retained".to_string());
        }
        if let Some(value) = &self.provider_request_id {
            safe_ascii_token(value, "provider_request_id", 128)?;
        }
        if let Some(value) = &self.provider_error_code {
            safe_ascii_token(value, "provider_error_code", 64)?;
            if !provider_code_in(RETRYABLE_PROVIDER_CODES, value)
                && !provider_code_in(NON_RETRYABLE_PROVIDER_CODES, value)
            {
                return Err("provider_error_code is not allowlisted".to_string());
            }
            if !self.code_present {
                return Err(
                    "code_present must be true when provider_error_code is retained".to_string(),
                );
            }
        }
        Ok(())
    }

    fn canonical_disposition(&self) -> Option<ProviderRecoveryDisposition> {
        // Keep this projection aligned with the canonical diagnostic carried by
        // the worker. In particular, independently typed 401/403 evidence is
        // actionable, while another status still needs its category/code facts
        // to classify it; the durable record must not invent a second policy.
        match self.http_status {
            Some(408 | 429 | 500..=599) => {
                return Some(ProviderRecoveryDisposition::Retryable);
            }
            Some(401 | 403) => return None,
            _ => {}
        }
        if let Some(code) = self.provider_error_code.as_deref() {
            if provider_code_in(RETRYABLE_PROVIDER_CODES, code) {
                return Some(ProviderRecoveryDisposition::Retryable);
            }
            if provider_code_in(NON_RETRYABLE_PROVIDER_CODES, code) {
                return None;
            }
        }
        match self.category.as_str() {
            "timeout" | "transport" | "rate_limit" => Some(ProviderRecoveryDisposition::Retryable),
            "authentication" | "context" => None,
            "response" if self.boundary == "local" => None,
            "response" | "provider" | "redacted_unknown" => {
                Some(ProviderRecoveryDisposition::Unknown)
            }
            _ => None,
        }
    }
}

/// Restart-safe provider deferral for one unsucceeded workstream epoch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRecovery {
    pub workstream_id: String,
    pub failure_epoch: u32,
    pub disposition: ProviderRecoveryDisposition,
    pub facts: ProviderRecoveryFacts,
    pub cumulative_failure_count: u32,
    pub deferral_count: u32,
    /// Configured maximum deferrals for this epoch.
    pub deferral_limit: u32,
    /// Monotonic fence shared by timer and provider-health wakes.
    pub generation: u32,
    pub not_before: DateTime<Utc>,
    pub epoch_started_at: DateTime<Utc>,
    pub elapsed_ms: u64,
    pub slo_deadline: DateTime<Utc>,
    /// Stable digest of the deferral or health-wake event last converged.
    pub idempotency_key: String,
    /// Exact failed attempt that produced this deferral generation.
    pub source_attempt_id: String,
    /// Exact due assignment admitted through the lease fence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_assignment_attempt_id: Option<String>,
    /// Authenticated health event last applied, for duplicate delivery convergence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_event_id: Option<String>,
}

impl ProviderRecovery {
    pub fn validate(&self) -> Result<(), String> {
        safe_identity(&self.workstream_id, "workstream_id")?;
        safe_identity(&self.source_attempt_id, "source_attempt_id")?;
        if let Some(value) = &self.due_assignment_attempt_id {
            safe_identity(value, "due_assignment_attempt_id")?;
        }
        if let Some(value) = &self.health_event_id {
            safe_identity(value, "health_event_id")?;
        }
        safe_hex_digest(&self.idempotency_key)?;
        self.facts.validate()?;
        if self.facts.canonical_disposition() != Some(self.disposition) {
            return Err(
                "provider recovery disposition conflicts with actionable provider facts"
                    .to_string(),
            );
        }
        if self.failure_epoch == 0
            || self.cumulative_failure_count == 0
            || self.deferral_count == 0
            || self.deferral_limit == 0
            || self.generation == 0
        {
            return Err("provider recovery counters must be positive".to_string());
        }
        if self.failure_epoch > MAX_PROVIDER_RECOVERY_COUNT
            || self.cumulative_failure_count > MAX_PROVIDER_RECOVERY_COUNT
            || self.deferral_count > MAX_PROVIDER_RECOVERY_COUNT
            || self.deferral_limit > MAX_PROVIDER_RECOVERY_COUNT
            || self.generation > self.deferral_limit.saturating_mul(2)
            || self.deferral_count > self.deferral_limit
            || self.deferral_count > self.cumulative_failure_count
        {
            return Err("provider recovery counters exceed their durable bound".to_string());
        }
        if self.slo_deadline <= self.epoch_started_at {
            return Err("provider recovery SLO deadline must follow its epoch start".to_string());
        }
        if self.not_before < self.epoch_started_at || self.not_before > self.slo_deadline {
            return Err("provider recovery not-before must be within its SLO window".to_string());
        }
        let window_ms = self
            .slo_deadline
            .signed_duration_since(self.epoch_started_at)
            .num_milliseconds();
        if window_ms < 0 || self.elapsed_ms > u64::try_from(window_ms).unwrap_or_default() {
            return Err("provider recovery elapsed time exceeds its SLO window".to_string());
        }
        Ok(())
    }

    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        now >= self.not_before && now < self.slo_deadline
    }

    pub fn slo_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.slo_deadline
    }

    pub fn authorizes_attempt(&self, attempt_id: Option<&str>) -> bool {
        self.validate().is_ok()
            && attempt_id.is_some()
            && self.due_assignment_attempt_id.as_deref() == attempt_id
    }
}

const RETRYABLE_PROVIDER_CODES: &[&str] = &[
    "api_error",
    "internal_error",
    "overloaded",
    "overloaded_error",
    "rate_limit",
    "rate_limit_error",
    "rate_limit_exceeded",
    "rate_limit_exceeded.v2",
    "request_timeout",
    "request_timed_out",
    "server_error",
    "timeout",
    "timeout_error",
    "too_many_requests",
    "unavailable",
];

const NON_RETRYABLE_PROVIDER_CODES: &[&str] = &[
    "authentication_error",
    "billing_not_active",
    "context_length_exceeded",
    "context_window_exceeded",
    "entitlement_required",
    "insufficient_permissions",
    "insufficient_quota",
    "invalid_api_key",
    "invalid_request_error",
    "malformed_sse",
    "malformed_stream",
    "max_tokens_exceeded",
    "model_not_found",
    "not_found_error",
    "permission_denied",
    "permission_error",
    "prompt_too_long",
    "provider_error",
    "quota_exceeded",
    "request_too_large",
    "unauthorized",
    "usage_limit",
    "usage_limit_reached",
];

fn provider_code_in(allowlist: &[&str], value: &str) -> bool {
    allowlist
        .iter()
        .any(|allowed| value.eq_ignore_ascii_case(allowed))
}

fn safe_identity(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.len() > MAX_PROVIDER_RECOVERY_ID_BYTES
        || value.contains("-->")
        || value.chars().any(|character| {
            character.is_control()
                || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
    {
        return Err(format!(
            "{field} must be 1..={MAX_PROVIDER_RECOVERY_ID_BYTES} safe UTF-8 bytes"
        ));
    }
    Ok(())
}

fn safe_ascii_token(value: &str, field: &str, max: usize) -> Result<(), String> {
    if value.is_empty()
        || value.len() > max
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err(format!("{field} must be a bounded ASCII token"));
    }
    Ok(())
}

fn safe_hex_digest(value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("provider recovery idempotency_key must be a SHA-256 hex digest".to_string());
    }
    Ok(())
}

fn allowed(value: &str, field: &str, choices: &[&str]) -> Result<(), String> {
    if choices.contains(&value) {
        Ok(())
    } else {
        Err(format!("provider recovery {field} is not allowlisted"))
    }
}
