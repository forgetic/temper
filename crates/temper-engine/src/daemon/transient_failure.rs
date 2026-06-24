// SPDX-License-Identifier: MPL-2.0

//! Bounded escalation for repeated transient worker/agent failures.
//!
//! A single transient worker result is still treated as retryable: the daemon
//! drops it and waits for the normal scan/backstop path to re-feed the work.
//! This module records those drops by the work item's correlation key and, once
//! the same item has failed repeatedly, converts the next result into a
//! human-attention failure with a sanitized diagnostic. The forge applier then
//! labels/comments the source artifact instead of letting the retry loop hide a
//! persistent provider/configuration problem.

use std::collections::BTreeMap;

use temper_engine_io::EngineTime;
use temper_protocol_worker::{Failure, FailureClass, JobResult, ResultStatus};

use crate::InFlightJob;

/// Total transient failures allowed for one work item before operator attention
/// is required. The threshold includes the failure that trips escalation.
pub(super) const TRANSIENT_FAILURE_ESCALATION_THRESHOLD: usize = 3;

const MAX_DIAGNOSTIC_MESSAGE_CHARS: usize = 600;
const REDACTED: &str = "[REDACTED]";

#[derive(Debug, Clone)]
pub(super) struct TransientFailureHistory {
    threshold: usize,
    records: BTreeMap<String, TransientFailureRecord>,
}

impl Default for TransientFailureHistory {
    fn default() -> Self {
        Self::new(TRANSIENT_FAILURE_ESCALATION_THRESHOLD)
    }
}

impl TransientFailureHistory {
    pub(super) fn new(threshold: usize) -> Self {
        Self {
            threshold: threshold.max(1),
            records: BTreeMap::new(),
        }
    }

    /// Records one accepted transient failure for `job` and returns whether this
    /// failure should still be retried by scan re-feed or escalated for humans.
    pub(super) fn record_transient_failure(
        &mut self,
        job: &InFlightJob,
        result: &JobResult,
        now: EngineTime,
    ) -> TransientFailureDecision {
        let work_key = WorkFailureKey::for_job(job);
        let raw_message = result
            .failure
            .as_ref()
            .map(|failure| failure.message.as_str())
            .unwrap_or_default();
        let record = self
            .records
            .entry(work_key.key.clone())
            .or_insert_with(|| TransientFailureRecord::new(work_key));
        record.record(result.worker_id.clone(), raw_message, now);

        if record.attempt_count >= self.threshold {
            TransientFailureDecision::Escalate {
                diagnostic: record.diagnostic(self.threshold),
            }
        } else {
            TransientFailureDecision::RetryLater {
                attempt_count: record.attempt_count,
                threshold: self.threshold,
            }
        }
    }

    /// A non-transient terminal result proves the retry series is over.
    pub(super) fn clear_for_job(&mut self, job: &InFlightJob) {
        let key = WorkFailureKey::for_job(job).key;
        self.records.remove(&key);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TransientFailureDecision {
    RetryLater {
        attempt_count: usize,
        threshold: usize,
    },
    Escalate {
        diagnostic: TransientFailureDiagnostic,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TransientFailureDiagnostic {
    pub(super) correlation_key: Option<String>,
    pub(super) attempt_count: usize,
    pub(super) threshold: usize,
    pub(super) worker_id: String,
    pub(super) first_failure_at: EngineTime,
    pub(super) last_failure_at: EngineTime,
    pub(super) provider_error_class: String,
    pub(super) provider_error_message: String,
}

impl TransientFailureDiagnostic {
    pub(super) fn apply_to_result(&self, mut result: JobResult) -> JobResult {
        result.status = ResultStatus::Failure;
        result.repos.clear();
        result.verdict = None;
        result.body = None;
        result.children.clear();
        result.failure = Some(Failure {
            class: FailureClass::Permanent,
            message: self.failure_message(),
        });
        result.summary = Some(format!(
            "transient agent failures exceeded retry threshold ({}/{})",
            self.attempt_count, self.threshold
        ));
        result
    }

    fn failure_message(&self) -> String {
        let correlation_key = self.correlation_key.as_deref().unwrap_or("unknown");
        format!(
            "Transient provider/model failures reached the automatic retry threshold; operator attention is required before this work item is requeued.\n\n\
             attempt_count: `{}`\n\
             retry_threshold: `{}`\n\
             worker_id: `{}`\n\
             correlation_key: `{}`\n\
             first_failure_engine_time_ns: `{}`\n\
             last_failure_engine_time_ns: `{}`\n\
             provider_error_class: `{}`\n\
             provider_error_message: `{}`",
            self.attempt_count,
            self.threshold,
            escape_code_span(&self.worker_id),
            escape_code_span(correlation_key),
            self.first_failure_at.as_nanos(),
            self.last_failure_at.as_nanos(),
            escape_code_span(&self.provider_error_class),
            escape_code_span(&self.provider_error_message)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TransientFailureRecord {
    work_key: WorkFailureKey,
    attempt_count: usize,
    first_failure_at: EngineTime,
    last_failure_at: EngineTime,
    last_worker_id: String,
    last_provider_error_class: String,
    last_provider_error_message: String,
}

impl TransientFailureRecord {
    fn new(work_key: WorkFailureKey) -> Self {
        Self {
            work_key,
            attempt_count: 0,
            first_failure_at: EngineTime::ZERO,
            last_failure_at: EngineTime::ZERO,
            last_worker_id: String::new(),
            last_provider_error_class: String::new(),
            last_provider_error_message: String::new(),
        }
    }

    fn record(&mut self, worker_id: String, raw_message: &str, now: EngineTime) {
        self.attempt_count += 1;
        if self.attempt_count == 1 {
            self.first_failure_at = now;
        }
        self.last_failure_at = now;
        self.last_worker_id = worker_id;
        self.last_provider_error_class = provider_error_class(raw_message).to_string();
        self.last_provider_error_message = sanitize_provider_error_message(raw_message);
    }

    fn diagnostic(&self, threshold: usize) -> TransientFailureDiagnostic {
        TransientFailureDiagnostic {
            correlation_key: self.work_key.correlation_key.clone(),
            attempt_count: self.attempt_count,
            threshold,
            worker_id: self.last_worker_id.clone(),
            first_failure_at: self.first_failure_at,
            last_failure_at: self.last_failure_at,
            provider_error_class: self.last_provider_error_class.clone(),
            provider_error_message: self.last_provider_error_message.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkFailureKey {
    key: String,
    correlation_key: Option<String>,
}

impl WorkFailureKey {
    fn for_job(job: &InFlightJob) -> Self {
        let correlation_key = payload_coordination_key(&job.job_payload).map(str::to_string);
        let key = correlation_key
            .as_ref()
            .map(|correlation_key| format!("{}|{}", job.repo, correlation_key))
            .unwrap_or_else(|| job.job_id.clone());
        Self {
            key,
            correlation_key,
        }
    }
}

fn payload_coordination_key(job_payload: &serde_json::Value) -> Option<&str> {
    job_payload
        .get("workspace")
        .and_then(|workspace| workspace.get("coordination_key"))
        .and_then(serde_json::Value::as_str)
}

fn provider_error_class(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("rate_limit")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || status_numbers(&lower).any(|status| status == 429)
    {
        return "rate_limit";
    }
    if lower.contains("timeout") || lower.contains("timed out") || lower.contains("stalled") {
        return "timeout";
    }
    if lower.contains("overloaded") || lower.contains("service_unavailable") {
        return "overloaded";
    }
    if status_numbers(&lower).any(|status| (500..=599).contains(&status))
        || lower.contains("server_error")
        || lower.contains("internal_server_error")
        || lower.contains("bad gateway")
    {
        return "server_error";
    }
    if lower.contains("connection")
        || lower.contains("dns")
        || lower.contains("tls")
        || lower.contains("transport")
        || lower.contains("broken pipe")
        || lower.contains("eof")
    {
        return "transport";
    }
    if lower.contains("retryable-provider:") {
        return "retryable_provider";
    }
    "transient_provider_error"
}

fn status_numbers(message: &str) -> impl Iterator<Item = u16> + '_ {
    message
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|token| token.len() == 3)
        .filter_map(|token| token.parse::<u16>().ok())
}

fn sanitize_provider_error_message(message: &str) -> String {
    let collapsed = collapse_message(message);
    let redacted = redact_secret_tokens(&collapsed);
    truncate_chars(&redacted, MAX_DIAGNOSTIC_MESSAGE_CHARS)
}

fn collapse_message(message: &str) -> String {
    let mut collapsed = String::new();
    let mut previous_space = false;
    for ch in message.chars() {
        let ch = if ch.is_control() || ch.is_whitespace() {
            ' '
        } else if ch == '`' {
            '\''
        } else {
            ch
        };
        if ch == ' ' {
            if !previous_space {
                collapsed.push(ch);
                previous_space = true;
            }
        } else {
            collapsed.push(ch);
            previous_space = false;
        }
    }
    collapsed.trim().to_string()
}

fn redact_secret_tokens(message: &str) -> String {
    let mut redacted = Vec::new();
    let mut redact_next = false;

    for token in message.split_whitespace() {
        if redact_next {
            let normalized = normalize_token(token);
            if normalized == "bearer" {
                redacted.push("Bearer".to_string());
                redact_next = true;
            } else {
                redacted.push(REDACTED.to_string());
                redact_next = false;
            }
            continue;
        }

        let normalized = normalize_token(token);
        if secret_key_prefix(&normalized).is_some() {
            let key = secret_key_prefix(&normalized).unwrap_or("secret");
            redacted.push(format!("{key}={REDACTED}"));
            redact_next = token_ends_with_key_separator(token);
            continue;
        }

        if looks_like_secret_value(&normalized) {
            redacted.push(REDACTED.to_string());
        } else {
            redacted.push(token.to_string());
        }
    }

    redacted.join(" ")
}

fn normalize_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .to_ascii_lowercase()
}

fn secret_key_prefix(normalized: &str) -> Option<&'static str> {
    for key in [
        "authorization",
        "api_key",
        "apikey",
        "x-api-key",
        "access_token",
        "refresh_token",
        "id_token",
        "client_secret",
        "password",
        "passwd",
        "secret",
        "token",
    ] {
        if normalized == key
            || normalized.starts_with(&format!("{key}="))
            || normalized.starts_with(&format!("{key}:"))
            || normalized.starts_with(&format!("{key}\":"))
        {
            return Some(key);
        }
    }
    None
}

fn token_ends_with_key_separator(token: &str) -> bool {
    token.ends_with(':') || token.ends_with('=')
}

fn looks_like_secret_value(normalized: &str) -> bool {
    normalized.len() >= 12
        && (normalized.starts_with("sk-")
            || normalized.starts_with("sk_")
            || normalized.starts_with("ghp_")
            || normalized.starts_with("github_pat_")
            || normalized.starts_with("glpat-")
            || normalized.starts_with("xoxb-")
            || normalized.starts_with("xoxp-"))
}

fn truncate_chars(message: &str, max_chars: usize) -> String {
    if message.chars().count() <= max_chars {
        return message.to_string();
    }
    let mut truncated = message.chars().take(max_chars).collect::<String>();
    truncated.push_str("…[truncated]");
    truncated
}

fn escape_code_span(value: &str) -> String {
    value.replace('`', "'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use temper_protocol_worker::{Artifact, Failure, WorkspaceManifest};

    fn job() -> InFlightJob {
        InFlightJob {
            job_id: "ai/temper/issue-114/engineer/code_ready".to_string(),
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            artifact: Artifact {
                item: json!(114),
                kind: "issue".to_string(),
            },
            job_payload: json!({
                "workspace": WorkspaceManifest {
                    coordination_key: "pr-for-code-114".to_string(),
                    repos: Vec::new(),
                }
            }),
        }
    }

    fn transient_result(worker_id: &str, message: &str) -> JobResult {
        JobResult {
            protocol_version: temper_protocol_worker::WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.to_string(),
            job_id: "ai/temper/issue-114/engineer/code_ready".to_string(),
            status: ResultStatus::Failure,
            repos: Vec::new(),
            verdict: None,
            body: None,
            children: Vec::new(),
            failure: Some(Failure {
                class: FailureClass::Transient,
                message: message.to_string(),
            }),
            summary: None,
            details: None,
        }
    }

    #[test]
    fn repeated_transient_failures_escalate_at_threshold() {
        let mut history = TransientFailureHistory::new(3);
        let job = job();

        assert_eq!(
            history.record_transient_failure(
                &job,
                &transient_result("worker-a", "HTTP 503 service_unavailable"),
                EngineTime::from_nanos(10),
            ),
            TransientFailureDecision::RetryLater {
                attempt_count: 1,
                threshold: 3,
            }
        );
        assert!(matches!(
            history.record_transient_failure(
                &job,
                &transient_result("worker-b", "HTTP 503 service_unavailable"),
                EngineTime::from_nanos(20),
            ),
            TransientFailureDecision::RetryLater {
                attempt_count: 2,
                threshold: 3,
            }
        ));

        let decision = history.record_transient_failure(
            &job,
            &transient_result("worker-c", "retryable-provider: HTTP 503 service_unavailable"),
            EngineTime::from_nanos(30),
        );
        let TransientFailureDecision::Escalate { diagnostic } = decision else {
            panic!("third transient failure should escalate: {decision:?}");
        };
        assert_eq!(diagnostic.attempt_count, 3);
        assert_eq!(diagnostic.worker_id, "worker-c");
        assert_eq!(diagnostic.correlation_key.as_deref(), Some("pr-for-code-114"));
        assert_eq!(diagnostic.provider_error_class, "overloaded");
        assert_eq!(diagnostic.first_failure_at.as_nanos(), 10);
        assert_eq!(diagnostic.last_failure_at.as_nanos(), 30);

        let escalated = diagnostic.apply_to_result(transient_result("worker-c", "ignored"));
        let failure = escalated.failure.expect("escalated failure details");
        assert_eq!(failure.class, FailureClass::Permanent);
        assert!(failure.message.contains("attempt_count: `3`"));
        assert!(failure.message.contains("worker_id: `worker-c`"));
        assert!(failure.message.contains("provider_error_class: `overloaded`"));
    }

    #[test]
    fn sanitized_diagnostic_redacts_common_secret_shapes() {
        let raw = concat!(
            "retryable-provider: HTTP 429 rate_limit ",
            "Authorization: Bearer sk-live-secret-value ",
            "api_key=plain-secret ",
            "refresh_token:\"rt-secret\" ",
            "github_pat_0123456789abcdef ",
            "body={\"prompt\":\"full request should not matter\"}"
        );

        let sanitized = sanitize_provider_error_message(raw);

        assert!(sanitized.contains("retryable-provider: HTTP 429 rate_limit"));
        assert!(sanitized.contains("authorization=[REDACTED]"));
        assert!(sanitized.contains("api_key=[REDACTED]"));
        assert!(sanitized.contains("refresh_token=[REDACTED]"));
        assert!(!sanitized.contains("sk-live-secret-value"));
        assert!(!sanitized.contains("plain-secret"));
        assert!(!sanitized.contains("rt-secret"));
        assert!(!sanitized.contains("github_pat_0123456789abcdef"));

        let diagnostic = TransientFailureDiagnostic {
            correlation_key: Some("pr-for-code-114".to_string()),
            attempt_count: 3,
            threshold: 3,
            worker_id: "worker-a".to_string(),
            first_failure_at: EngineTime::from_nanos(1),
            last_failure_at: EngineTime::from_nanos(2),
            provider_error_class: provider_error_class(raw).to_string(),
            provider_error_message: sanitized,
        };
        let rendered = diagnostic.failure_message();
        assert!(rendered.contains("provider_error_class: `rate_limit`"));
        assert!(!rendered.contains("sk-live-secret-value"));
        assert!(!rendered.contains("plain-secret"));
        assert!(!rendered.contains("rt-secret"));
    }
}
