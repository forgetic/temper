// SPDX-License-Identifier: MPL-2.0

//! Library-only Forgejo/Gitea/GitHub webhook intake helpers for daemon wake scans.

mod payload;
mod trigger_facts;

use std::{collections::BTreeMap, fmt};

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use temper_forge::{ChangeHint, ChangeKind, Forge, HintArtifactKind, HintTarget, RepositoryPath};
use temper_workflow::{CompiledWorkflow, ValidatedWorkflow, is_heartbeat_only_body_change};

use crate::{Daemon, RoleFeedMode, RoleFeedTarget};
use payload::{is_ci_event, is_review_event, parse_repo, parse_target};
pub(crate) use trigger_facts::parse_trigger_facts;
use trigger_facts::{wake_artifact_kind, wake_queue};

/// Errors returned while verifying or parsing a webhook delivery.
#[derive(Debug, Eq, PartialEq)]
pub enum WebhookError {
    /// The signature header was missing or did not match the body HMAC.
    InvalidSignature,
    /// The payload could not be parsed into a change hint.
    BadPayload(String),
}

impl fmt::Display for WebhookError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WebhookError::InvalidSignature => write!(formatter, "invalid webhook signature"),
            WebhookError::BadPayload(message) => {
                write!(formatter, "bad webhook payload: {message}")
            }
        }
    }
}

impl std::error::Error for WebhookError {}

/// Whether an accepted webhook must schedule work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebhookDisposition {
    Schedule,
    SuppressHeartbeat,
}

/// Fully verified and conservatively classified webhook delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedWebhook {
    pub hint: ChangeHint,
    pub disposition: WebhookDisposition,
}

/// Webhook intake configuration: the shared webhook secret plus the configured
/// daemon role-feed targets eligible for wake scans.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebhookConfig {
    pub secret: String,
    pub targets: Vec<RoleFeedTarget>,
}

/// Verifies the Forgejo/Gitea/GitHub HMAC-SHA256 signature for a webhook body.
pub fn verify_webhook_signature(
    headers: &BTreeMap<String, String>,
    body: &[u8],
    secret: &str,
) -> Result<(), WebhookError> {
    let supplied = headers
        .get("x-forgejo-signature")
        .or_else(|| headers.get("x-gitea-signature"))
        .or_else(|| headers.get("x-hub-signature-256"))
        .ok_or(WebhookError::InvalidSignature)?;
    let supplied = supplied.strip_prefix("sha256=").unwrap_or(supplied);
    let supplied_bytes = decode_hex(supplied).ok_or(WebhookError::InvalidSignature)?;
    let expected = hmac_sha256(secret.as_bytes(), body);

    if constant_time_eq(&supplied_bytes, &expected) {
        Ok(())
    } else {
        Err(WebhookError::InvalidSignature)
    }
}

/// Returns the provider event header value, or `unknown` when absent.
pub fn webhook_event(headers: &BTreeMap<String, String>) -> &str {
    headers
        .get("x-forgejo-event")
        .or_else(|| headers.get("x-gitea-event"))
        .map(String::as_str)
        .unwrap_or("unknown")
}

/// Parses a webhook JSON body into a provider-neutral change hint.
pub fn parse_change_hint(body: &[u8], event: &str) -> Result<ChangeHint, WebhookError> {
    Ok(parse_webhook(body, event)?.hint)
}

fn parse_webhook(body: &[u8], event: &str) -> Result<VerifiedWebhook, WebhookError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| WebhookError::BadPayload(format!("invalid JSON payload: {error}")))?;
    let repo = parse_repo(&value)?;
    let target = parse_target(&value, event);
    let mut change = classify_change(&value, event);
    if target == HintTarget::Repository && item_event(event) {
        // An item-like event with no unambiguous artifact address must force the
        // broad fallback rather than preserve a misleading targeted change.
        change = ChangeKind::Unknown;
    }
    let hint = ChangeHint {
        repo,
        target,
        change,
    };
    let disposition = classify_disposition(&value, event, &hint);
    Ok(VerifiedWebhook { hint, disposition })
}

fn classify_change(value: &Value, event: &str) -> ChangeKind {
    if is_review_event(event) {
        return ChangeKind::Review;
    }
    if is_ci_event(event) {
        return ChangeKind::Ci;
    }
    match event {
        "push" => ChangeKind::Push,
        "issue_comment" | "pull_request_comment" | "comment" => ChangeKind::Comment,
        "issue_dependency"
        | "issue_dependencies"
        | "pull_request_dependency"
        | "pull_request_dependencies" => ChangeKind::Dependency,
        "issues" | "issue" | "pull_request" | "pull_request_sync" => classify_item_change(value),
        _ => ChangeKind::Unknown,
    }
}

fn classify_item_change(value: &Value) -> ChangeKind {
    match value.pointer("/action").and_then(Value::as_str) {
        Some("opened" | "created") => ChangeKind::Created,
        Some("closed" | "reopened" | "merged") => ChangeKind::State,
        Some("labeled" | "unlabeled" | "label_updated") => ChangeKind::Label,
        Some("assigned" | "unassigned") => ChangeKind::Assignee,
        Some("dependency_added" | "dependency_removed") => ChangeKind::Dependency,
        Some("edited") => explicit_edit_kind(value),
        Some("synchronize" | "synchronized") => ChangeKind::Edited,
        _ => ChangeKind::Edited,
    }
}

fn explicit_edit_kind(value: &Value) -> ChangeKind {
    let Some(changes) = value.pointer("/changes").and_then(Value::as_object) else {
        return ChangeKind::Edited;
    };
    if changes.len() != 1 {
        return ChangeKind::Edited;
    }
    match changes.keys().next().map(String::as_str) {
        Some("body") => ChangeKind::Body,
        Some("title") => ChangeKind::Title,
        Some("state") => ChangeKind::State,
        Some("label" | "labels") => ChangeKind::Label,
        Some("dependency" | "dependencies") => ChangeKind::Dependency,
        Some("assignee" | "assignees") => ChangeKind::Assignee,
        _ => ChangeKind::Edited,
    }
}

fn classify_disposition(value: &Value, event: &str, hint: &ChangeHint) -> WebhookDisposition {
    if !matches!(event, "issues" | "issue" | "pull_request")
        || value.pointer("/action").and_then(Value::as_str) != Some("edited")
        || !matches!(hint.target, HintTarget::Artifact { .. })
    {
        return WebhookDisposition::Schedule;
    }
    let Some(changes) = value.pointer("/changes").and_then(Value::as_object) else {
        return WebhookDisposition::Schedule;
    };
    if changes.len() != 1 || !changes.contains_key("body") {
        return WebhookDisposition::Schedule;
    }
    let Some(old_body) = value.pointer("/changes/body/from").and_then(Value::as_str) else {
        return WebhookDisposition::Schedule;
    };
    let new_body = value
        .pointer("/changes/body/to")
        .and_then(Value::as_str)
        .or_else(|| match hint.target {
            HintTarget::Artifact {
                kind: HintArtifactKind::Issue,
                ..
            } => value.pointer("/issue/body").and_then(Value::as_str),
            HintTarget::Artifact {
                kind: HintArtifactKind::PullRequest,
                ..
            } => value.pointer("/pull_request/body").and_then(Value::as_str),
            HintTarget::Repository => None,
        });

    if new_body.is_some_and(|new_body| is_heartbeat_only_body_change(old_body, new_body)) {
        WebhookDisposition::SuppressHeartbeat
    } else {
        WebhookDisposition::Schedule
    }
}

fn item_event(event: &str) -> bool {
    matches!(
        event,
        "issues"
            | "issue"
            | "pull_request"
            | "pull_request_sync"
            | "issue_comment"
            | "pull_request_comment"
            | "comment"
            | "issue_dependency"
            | "issue_dependencies"
            | "pull_request_dependency"
            | "pull_request_dependencies"
    ) || is_review_event(event)
}

/// Verifies a webhook signature and parses its body into a classified delivery.
pub fn parse_verified_webhook(
    headers: &BTreeMap<String, String>,
    body: &[u8],
    secret: &str,
) -> Result<VerifiedWebhook, WebhookError> {
    // Verification deliberately precedes JSON parsing and all classification.
    verify_webhook_signature(headers, body, secret)?;
    parse_webhook(body, webhook_event(headers))
}

pub(crate) fn webhook_accepted_log_line(hint: &ChangeHint) -> String {
    let target = match hint.target {
        HintTarget::Repository => "repository".to_string(),
        HintTarget::Artifact { kind, number } => {
            format!("{}#{}", artifact_token(kind), number)
        }
    };

    format!(
        "engine: webhook accepted repo={}/{} target={} change={:?}",
        hint.repo.owner, hint.repo.name, target, hint.change
    )
}

fn webhook_wake_scan_log_line(repo: &RepositoryPath, enqueued: usize) -> String {
    format!(
        "engine: webhook wake scan repo={}/{} enqueued={enqueued}",
        repo.owner, repo.name
    )
}

fn artifact_token(kind: HintArtifactKind) -> &'static str {
    match kind {
        HintArtifactKind::Issue => "issue",
        HintArtifactKind::PullRequest => "pull_request",
    }
}

/// Computes the lowercase hex HMAC-SHA256 signature for a webhook body.
pub fn webhook_signature(secret: &str, body: &[u8]) -> String {
    encode_hex(&hmac_sha256(secret.as_bytes(), body))
}

/// Handles one verified webhook delivery as a wake accelerator for matching targets.
#[allow(clippy::too_many_arguments)]
pub async fn handle_webhook<F: Forge + ?Sized>(
    daemon: &Daemon,
    forge: &F,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    config: &WebhookConfig,
    headers: &BTreeMap<String, String>,
    body: &[u8],
) -> Result<usize, WebhookError> {
    let verified = parse_verified_webhook(headers, body, &config.secret)?;
    let hint = &verified.hint;
    let line = webhook_accepted_log_line(hint);
    tracing::debug!("{line}");

    if verified.disposition == WebhookDisposition::SuppressHeartbeat {
        tracing::debug!(
            target: "temper::engine",
            service = "engine",
            repo = %format!("{}/{}", hint.repo.owner, hint.repo.name),
            wake.reason = "lease_heartbeat",
            wake.scope = "targeted",
            wake.outcome = "suppressed",
            wake.pending_target_count = 0,
            wake.in_flight_repository_count = 0,
            wake.queue_latency_ms = 0_u64,
            wake.execution_duration_ms = 0_u64,
            "engine: wake decision"
        );
        return Ok(0);
    }

    let repo =
        temper_log::strip_provider_scheme(&format!("{}/{}", hint.repo.owner, hint.repo.name))
            .to_string();
    let facts = parse_trigger_facts(body, webhook_event(headers), hint);
    if let Some(issue) = facts.issue_opened.as_ref() {
        let item = temper_log::WorkItemRef::issue(repo.clone(), issue.number);
        temper_log::emit::emit_issue_opened(temper_log::emit::IssueOpened {
            item: &item,
            author: &issue.author,
            title: &issue.title,
        });
    }
    if let Some(ci) = facts.ci_completed.as_ref() {
        let item = temper_log::WorkItemRef::pull_request(repo.clone(), ci.pr_number);
        temper_log::emit::emit_ci_completed(temper_log::emit::CiCompleted {
            item: &item,
            conclusion: &ci.conclusion,
            duration_ms: ci.duration_ms,
            trigger_source: Some("webhook"),
            detection_latency_ms: ci.completed_at.map(|completed_at| {
                u64::try_from(now.signed_duration_since(completed_at).num_milliseconds())
                    .unwrap_or(0)
            }),
            queue: None,
            role: None,
        });
    }

    Ok(run_wake_scan(
        daemon,
        forge,
        workflow,
        compiled,
        now,
        &config.targets,
        hint,
    )
    .await)
}

/// Runs the role-work wake scans for one accepted change hint.
///
/// Live webhook deliveries and local backend change sources share this path:
/// resolve the hinted repository, scan configured wake targets, and enqueue any
/// work found from fresh Forge state.
pub async fn run_wake_scan<F: Forge + ?Sized>(
    daemon: &Daemon,
    forge: &F,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    targets: &[RoleFeedTarget],
    hint: &ChangeHint,
) -> usize {
    let repository = match forge.get_repository_by_path(&hint.repo).await {
        Ok(Some(repository)) => repository,
        Ok(None) => return 0,
        Err(error) => {
            tracing::warn!(
                target: "engine",
                repo_owner = %hint.repo.owner,
                repo_name = %hint.repo.name,
                %error,
                "wake repository lookup failed"
            );
            return 0;
        }
    };

    let mut total = 0;
    let mut matched_target = false;
    for target in targets {
        if target.repo != repository.id {
            continue;
        }
        matched_target = true;

        match daemon
            .enqueue_scanned_role_work(
                forge,
                &target.repo,
                workflow,
                compiled,
                now,
                &target.role,
                RoleFeedMode::Wake,
            )
            .await
        {
            Ok(count) => total += count,
            Err(error) => tracing::warn!(
                target: "engine",
                repo = %target.repo,
                role = %target.role.as_str(),
                %error,
                "wake scan failed"
            ),
        }
    }

    if matched_target {
        if let HintTarget::Artifact { kind, number } = hint.target {
            let repo = temper_log::strip_provider_scheme(&format!(
                "{}/{}",
                hint.repo.owner, hint.repo.name
            ))
            .to_string();
            let item = match kind {
                HintArtifactKind::Issue => temper_log::WorkItemRef::issue(repo, number.get()),
                HintArtifactKind::PullRequest => {
                    temper_log::WorkItemRef::pull_request(repo, number.get())
                }
            };
            temper_log::emit::emit_wake_received(temper_log::emit::WakeReceived {
                item: &item,
                artifact_kind: wake_artifact_kind(kind),
                queue: wake_queue(kind, hint.change),
            });
        }
        let line = webhook_wake_scan_log_line(&hint.repo, total);
        tracing::debug!("{line}");
    }

    total
}

fn hmac_sha256(secret: &[u8], body: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(body);
    mac.finalize().into_bytes().to_vec()
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(raw: &str) -> Option<Vec<u8>> {
    if raw.len() % 2 != 0 {
        return None;
    }
    raw.as_bytes()
        .chunks(2)
        .map(|pair| Some((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let diff = left
        .iter()
        .zip(right.iter())
        .fold(0_u8, |acc, (left, right)| acc | (left ^ right));
    diff == 0
}

#[cfg(test)]
mod tests;
