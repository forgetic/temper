// SPDX-License-Identifier: MPL-2.0

//! Provider-recovery scan admission, actionable parking, and authenticated health wakes.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use temper_forge::{
    CreateComment, Forge, ForgeError, Issue, PullRequest, RepositoryId, UpdateIssue,
    UpdatePullRequest,
};
use temper_runner::ScanError;
use temper_workflow::{
    ArtifactSource, inspect_metadata_blocks, parse_metadata_block, replace_metadata_block,
};

use crate::WallClock;
use crate::forge_applier::provider_recovery::recovery_event_key;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProviderRecoveryAdmission {
    Admit,
    Suppress,
    Park(String),
}

pub(crate) fn body_has_provider_recovery_key(body: &str) -> bool {
    match inspect_metadata_blocks(body) {
        Ok(inspection) => inspection
            .blocks()
            .iter()
            .any(|span| body[span.start()..span.end()].contains("\"provider_recovery\"")),
        // An unterminated real block has no closed span to inspect. The parser
        // has already established that managed metadata is structurally
        // corrupt, so a raw recovery key must fail closed rather than making
        // the work dispatchable.
        Err(_) => body.contains("\"provider_recovery\""),
    }
}

pub(crate) fn provider_recovery_admission(
    body: &str,
    now: DateTime<Utc>,
) -> ProviderRecoveryAdmission {
    let metadata = match parse_metadata_block(body) {
        Ok(metadata) => metadata.unwrap_or_default(),
        Err(error) => {
            return if body_has_provider_recovery_key(body) {
                ProviderRecoveryAdmission::Park(format!(
                    "provider recovery metadata is corrupt: {error}; repair or remove the bounded provider_recovery record after inspecting the preserved workspace"
                ))
            } else {
                ProviderRecoveryAdmission::Admit
            };
        }
    };
    let Some(recovery) = metadata.provider_recovery else {
        return ProviderRecoveryAdmission::Admit;
    };
    if let Err(reason) = recovery.validate() {
        return ProviderRecoveryAdmission::Park(format!(
            "provider recovery metadata is corrupt: {reason}; repair the record after inspecting the preserved workspace and session ledger"
        ));
    }
    if recovery.slo_expired(now) {
        return ProviderRecoveryAdmission::Park(
            "provider recovery SLO expired; inspect the preserved workspace and provider configuration, then deliberately restore queue eligibility"
                .to_string(),
        );
    }
    if recovery.is_due(now) {
        ProviderRecoveryAdmission::Admit
    } else {
        ProviderRecoveryAdmission::Suppress
    }
}

pub(crate) async fn enforce_provider_recovery_admission<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    target: ArtifactSource,
    body: &str,
    now: DateTime<Utc>,
) -> Result<bool, ScanError> {
    match provider_recovery_admission(body, now) {
        ProviderRecoveryAdmission::Admit => Ok(true),
        ProviderRecoveryAdmission::Suppress => Ok(false),
        ProviderRecoveryAdmission::Park(reason) => {
            park_provider_recovery(forge, repo, target, &reason)
                .await
                .map_err(ScanError::Forge)?;
            Ok(false)
        }
    }
}

async fn park_provider_recovery<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    target: ArtifactSource,
    reason: &str,
) -> temper_forge::ForgeResult<()> {
    for _ in 0..3 {
        let Some(snapshot) = RecoveryArtifact::load(forge, repo, target).await? else {
            return Ok(());
        };
        if !snapshot.labels().iter().any(|label| label == "needs-human") {
            match snapshot.park(forge).await {
                Ok(_) => continue,
                Err(ForgeError::Conflict(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        let marker = provider_park_marker(target, snapshot.body());
        let comments = snapshot.comments(forge).await?;
        if comments
            .iter()
            .any(|comment| comment.body.contains(&marker))
        {
            return Ok(());
        }
        let body = format!(
            "Temper parked this workstream because durable provider recovery cannot continue safely.\n\n**Safe reason:** {}\n\n**Operator repair:** inspect the preserved coordination-scoped workspace and session ledger, repair the bounded `provider_recovery` metadata or provider configuration, then deliberately restore the queue label.\n\n{}",
            escape_markdown(reason),
            marker
        );
        snapshot.comment(forge, body).await?;
        return Ok(());
    }
    Err(ForgeError::Conflict(
        "provider recovery parking remained contended".to_string(),
    ))
}

fn provider_park_marker(target: ArtifactSource, body: &str) -> String {
    let target_identity = match target {
        ArtifactSource::Issue { number } => format!("issue:{}", number.get()),
        ArtifactSource::PullRequest { number } => format!("pull_request:{}", number.get()),
    };
    let recovery_identity = parse_metadata_block(body)
        .ok()
        .flatten()
        .and_then(|metadata| metadata.provider_recovery)
        .map(|recovery| {
            format!(
                "{}:{}:{}",
                recovery.failure_epoch, recovery.generation, recovery.idempotency_key
            )
        })
        .unwrap_or_else(|| "corrupt".to_string());
    let identity = format!("{target_identity}:{recovery_identity}");
    format!(
        "<!-- temper:comment-key=provider_recovery_park:{} -->",
        recovery_event_key(&identity, 1, 1, "park", "park")
    )
}

fn escape_markdown(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Authenticated, workstream-scoped provider-health wake request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderHealthSignal {
    pub workstream_id: String,
    pub failure_epoch: u32,
    pub expected_generation: u32,
    pub event_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderHealthWakeOutcome {
    Advanced,
    Duplicate,
    Stale,
    NotDeferred,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ProviderHealthWakeError {
    InvalidSignature,
    InvalidSignal(String),
    CorruptRecovery(String),
    ActiveAssignment,
    Forge(String),
}

/// Forge-backed capability owned by the authenticated host, not by workers.
pub struct ProviderHealthWaker<F: Forge + ?Sized> {
    forge: Arc<F>,
    secret: SecretString,
    clock: WallClock,
}

impl<F: Forge + ?Sized> ProviderHealthWaker<F> {
    pub fn new(forge: Arc<F>, secret: SecretString, clock: WallClock) -> Self {
        Self {
            forge,
            secret,
            clock,
        }
    }

    pub async fn advance(
        &self,
        repo: &RepositoryId,
        target: ArtifactSource,
        signal: &ProviderHealthSignal,
        signature: &str,
    ) -> Result<ProviderHealthWakeOutcome, ProviderHealthWakeError> {
        validate_health_signal(signal)?;
        verify_health_signature(self.secret.expose_secret(), signal, signature)?;
        for _ in 0..3 {
            let Some(snapshot) = RecoveryArtifact::load(self.forge.as_ref(), repo, target)
                .await
                .map_err(|error| ProviderHealthWakeError::Forge(error.to_string()))?
            else {
                return Ok(ProviderHealthWakeOutcome::NotDeferred);
            };
            let mut metadata = parse_metadata_block(snapshot.body())
                .map_err(|error| ProviderHealthWakeError::CorruptRecovery(error.to_string()))?
                .unwrap_or_default();
            let Some(recovery) = metadata.provider_recovery.as_mut() else {
                return Ok(ProviderHealthWakeOutcome::NotDeferred);
            };
            recovery
                .validate()
                .map_err(ProviderHealthWakeError::CorruptRecovery)?;
            if recovery.workstream_id != signal.workstream_id
                || recovery.failure_epoch != signal.failure_epoch
            {
                return Ok(ProviderHealthWakeOutcome::Stale);
            }
            if recovery.health_event_id.as_deref() == Some(signal.event_id.as_str()) {
                return Ok(
                    if signal.expected_generation.checked_add(1) == Some(recovery.generation) {
                        ProviderHealthWakeOutcome::Duplicate
                    } else {
                        ProviderHealthWakeOutcome::Stale
                    },
                );
            }
            if recovery.generation != signal.expected_generation {
                return Ok(ProviderHealthWakeOutcome::Stale);
            }
            if metadata.assignment.is_some() || metadata.lease.is_some() {
                return Err(ProviderHealthWakeError::ActiveAssignment);
            }
            let now = (self.clock)();
            if recovery.slo_expired(now) || recovery.is_due(now) {
                return Ok(ProviderHealthWakeOutcome::Stale);
            }
            let generation_limit = recovery.deferral_limit.saturating_mul(2);
            let next = recovery
                .generation
                .checked_add(1)
                .filter(|generation| *generation <= generation_limit)
                .ok_or_else(|| {
                    ProviderHealthWakeError::InvalidSignal(
                        "provider recovery wake generation exhausted".to_string(),
                    )
                })?;
            recovery.generation = next;
            recovery.not_before = now.max(recovery.epoch_started_at);
            recovery.due_assignment_attempt_id = None;
            recovery.health_event_id = Some(signal.event_id.clone());
            recovery.idempotency_key = recovery_event_key(
                &recovery.workstream_id,
                recovery.failure_epoch,
                next,
                &signal.event_id,
                "health",
            );
            recovery
                .validate()
                .map_err(ProviderHealthWakeError::CorruptRecovery)?;
            let body = replace_metadata_block(snapshot.body(), &metadata)
                .map_err(|error| ProviderHealthWakeError::CorruptRecovery(error.to_string()))?;
            match snapshot.update_body(self.forge.as_ref(), body).await {
                Ok(_) => return Ok(ProviderHealthWakeOutcome::Advanced),
                Err(ForgeError::Conflict(_)) => continue,
                Err(error) => return Err(ProviderHealthWakeError::Forge(error.to_string())),
            }
        }
        Err(ProviderHealthWakeError::Forge(
            "provider health wake remained contended".to_string(),
        ))
    }
}

pub fn provider_health_signature(secret: &str, signal: &ProviderHealthSignal) -> String {
    let body = serde_json::to_vec(signal).expect("provider health signal serializes");
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(&body);
    encode_hex(&mac.finalize().into_bytes())
}

fn verify_health_signature(
    secret: &str,
    signal: &ProviderHealthSignal,
    signature: &str,
) -> Result<(), ProviderHealthWakeError> {
    if secret.is_empty() {
        return Err(ProviderHealthWakeError::InvalidSignature);
    }
    let supplied = decode_hex(signature.strip_prefix("sha256=").unwrap_or(signature))
        .ok_or(ProviderHealthWakeError::InvalidSignature)?;
    let body = serde_json::to_vec(signal).expect("provider health signal serializes");
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(&body);
    mac.verify_slice(&supplied)
        .map_err(|_| ProviderHealthWakeError::InvalidSignature)
}

fn validate_health_signal(signal: &ProviderHealthSignal) -> Result<(), ProviderHealthWakeError> {
    if !safe_signal_identity(&signal.workstream_id)
        || !safe_signal_identity(&signal.event_id)
        || signal.failure_epoch == 0
        || signal.expected_generation == 0
    {
        return Err(ProviderHealthWakeError::InvalidSignal(
            "provider health signal has invalid bounded identity".to_string(),
        ));
    }
    Ok(())
}

fn safe_signal_identity(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 256
        && !value.contains("-->")
        && !value.chars().any(|character| {
            character.is_control()
                || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.is_ascii() || value.len() % 2 != 0 {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Some((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

enum RecoveryArtifact {
    Issue(Box<Issue>),
    PullRequest(Box<PullRequest>),
}

impl RecoveryArtifact {
    async fn load<F: Forge + ?Sized>(
        forge: &F,
        repo: &RepositoryId,
        target: ArtifactSource,
    ) -> temper_forge::ForgeResult<Option<Self>> {
        match target {
            ArtifactSource::Issue { number } => forge
                .get_issue_by_number(repo, number)
                .await
                .map(|value| value.map(|issue| Self::Issue(Box::new(issue)))),
            ArtifactSource::PullRequest { number } => forge
                .get_pull_request_by_number(repo, number)
                .await
                .map(|value| value.map(|pull| Self::PullRequest(Box::new(pull)))),
        }
    }

    fn body(&self) -> &str {
        match self {
            Self::Issue(issue) => &issue.body,
            Self::PullRequest(pull) => &pull.body,
        }
    }

    fn labels(&self) -> &[String] {
        match self {
            Self::Issue(issue) => &issue.labels,
            Self::PullRequest(pull) => &pull.labels,
        }
    }

    async fn park<F: Forge + ?Sized>(&self, forge: &F) -> temper_forge::ForgeResult<()> {
        match self {
            Self::Issue(issue) => forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        add_labels: vec!["needs-human".to_string()],
                        remove_labels: vec!["ready".to_string(), "in-progress".to_string()],
                        remove_assignees: issue.assignees.clone(),
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
                .map(|_| ()),
            Self::PullRequest(pull) => forge
                .update_pull_request(
                    &pull.id,
                    UpdatePullRequest {
                        add_labels: vec!["needs-human".to_string()],
                        remove_labels: vec!["ready".to_string(), "in-progress".to_string()],
                        remove_assignees: pull.assignees.clone(),
                        expected_version: Some(pull.version),
                        ..UpdatePullRequest::default()
                    },
                )
                .await
                .map(|_| ()),
        }
    }

    async fn update_body<F: Forge + ?Sized>(
        &self,
        forge: &F,
        body: String,
    ) -> temper_forge::ForgeResult<()> {
        match self {
            Self::Issue(issue) => forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        body: Some(body),
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
                .map(|_| ()),
            Self::PullRequest(pull) => forge
                .update_pull_request(
                    &pull.id,
                    UpdatePullRequest {
                        body: Some(body),
                        expected_version: Some(pull.version),
                        ..UpdatePullRequest::default()
                    },
                )
                .await
                .map(|_| ()),
        }
    }

    async fn comments<F: Forge + ?Sized>(
        &self,
        forge: &F,
    ) -> temper_forge::ForgeResult<Vec<temper_forge::Comment>> {
        match self {
            Self::Issue(issue) => forge.list_issue_comments(&issue.id).await,
            Self::PullRequest(pull) => forge.list_pull_request_comments(&pull.id).await,
        }
    }

    async fn comment<F: Forge + ?Sized>(
        &self,
        forge: &F,
        body: String,
    ) -> temper_forge::ForgeResult<()> {
        match self {
            Self::Issue(issue) => forge
                .add_issue_comment(&issue.id, CreateComment { body })
                .await
                .map(|_| ()),
            Self::PullRequest(pull) => forge
                .add_pull_request_comment(&pull.id, CreateComment { body })
                .await
                .map(|_| ()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unterminated_provider_recovery_metadata_fails_closed() {
        let body = format!(
            "{}\n{{\"provider_recovery\":{{\"workstream_id\":\"deferred\"}}",
            temper_workflow::METADATA_BEGIN
        );
        assert!(body_has_provider_recovery_key(&body));
        assert!(matches!(
            provider_recovery_admission(&body, DateTime::<Utc>::UNIX_EPOCH),
            ProviderRecoveryAdmission::Park(reason) if reason.contains("corrupt")
        ));
    }
}
