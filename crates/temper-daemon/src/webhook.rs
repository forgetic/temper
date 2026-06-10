// SPDX-License-Identifier: MPL-2.0

//! Library-only Forgejo/Gitea webhook intake helpers for daemon wake scans.

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use temper_forge::{ChangeHint, ChangeKind, Forge, ItemNumber, RepositoryPath};
use temper_workflow::{CompiledWorkflow, ValidatedWorkflow};

use crate::{Daemon, RoleFeedMode, RoleFeedTarget};

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
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| WebhookError::BadPayload(format!("invalid JSON payload: {error}")))?;
    let repo = parse_repo(&value)?;
    let item = value
        .pointer("/pull_request/number")
        .and_then(Value::as_u64)
        .or_else(|| value.pointer("/issue/number").and_then(Value::as_u64))
        .map(ItemNumber::new);
    let kind = match event {
        "issues" | "issue" => ChangeKind::Issue,
        "pull_request" | "pull_request_sync" => ChangeKind::PullRequest,
        "issue_comment" | "pull_request_comment" | "comment" => ChangeKind::Comment,
        "pull_request_review"
        | "pull_request_review_approved"
        | "pull_request_review_rejected"
        | "pull_request_review_comment"
        | "pull_request_approved"
        | "pull_request_rejected"
        | "review" => ChangeKind::Review,
        "push" => ChangeKind::Push,
        "status" | "check_run" | "workflow_run" | "workflow_job" | "action_run_failure"
        | "action_run_recover" | "action_run_success" => ChangeKind::Ci,
        _ => ChangeKind::Unknown,
    };
    Ok(ChangeHint { repo, item, kind })
}

/// Verifies a webhook signature and parses its body into a change hint.
pub fn parse_verified_webhook(
    headers: &BTreeMap<String, String>,
    body: &[u8],
    secret: &str,
) -> Result<ChangeHint, WebhookError> {
    verify_webhook_signature(headers, body, secret)?;
    parse_change_hint(body, webhook_event(headers))
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
    let hint = parse_verified_webhook(headers, body, &config.secret)?;
    let repository = match forge.get_repository_by_path(&hint.repo).await {
        Ok(Some(repository)) => repository,
        Ok(None) => return Ok(0),
        Err(error) => {
            eprintln!(
                "temper-daemon: webhook repository lookup failed for repo={}/{}: {error}",
                hint.repo.owner, hint.repo.name
            );
            return Ok(0);
        }
    };

    let mut total = 0;
    for target in &config.targets {
        if target.repo != repository.id {
            continue;
        }

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
            Err(error) => eprintln!(
                "temper-daemon: webhook wake scan failed for repo={} role={}: {error}",
                target.repo,
                target.role.as_str()
            ),
        }
    }

    Ok(total)
}

fn parse_repo(value: &Value) -> Result<RepositoryPath, WebhookError> {
    if let Some(full) = value
        .pointer("/repository/full_name")
        .and_then(Value::as_str)
    {
        if let Some((owner, name)) = full.split_once('/') {
            return Ok(RepositoryPath::new(owner, name));
        }
    }

    let owner = value
        .pointer("/repository/owner/login")
        .or_else(|| value.pointer("/repository/owner/username"))
        .and_then(Value::as_str);
    let name = value.pointer("/repository/name").and_then(Value::as_str);
    match (owner, name) {
        (Some(owner), Some(name)) => Ok(RepositoryPath::new(owner, name)),
        _ => Err(WebhookError::BadPayload(
            "payload has no repository owner/name".into(),
        )),
    }
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
