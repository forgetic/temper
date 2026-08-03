//! Authenticated acquisition and fail-closed correlation of ordinary-failure
//! statements emitted by protected host-runner workflows.

use crate::types::ActionJobDto;
use crate::{ForgejoFailureEvidenceConfig, ForgejoForge, HttpClient, HttpMethod, HttpRequest};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use std::collections::BTreeSet;
use temper_forge_model::{
    CiFailureProofAttestation, CiFailureProofCoordinates, CiFailureProofSubject,
    CiFailureProofVerification, CiJob, CiJobConclusion, CiJobStatus, CiOrdinaryFailureCategory,
    CiVerifiedFailureProof, PullRequestId, RepositoryId,
};

const EVIDENCE_SCHEMA_VERSION: u16 = 1;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_RECORDS: usize = 128;
const MAX_STATEMENT_BYTES: usize = 8 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceResponse {
    schema_version: u16,
    records: Vec<SignedStatement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedStatement {
    statement: String,
    hmac_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FailureStatement {
    schema_version: u16,
    category: CiOrdinaryFailureCategory,
    repository_id: RepositoryId,
    #[serde(default)]
    pull_request_id: Option<PullRequestId>,
    commit_sha: String,
    run_id: String,
    job_id: String,
    attempt: String,
    task_id: String,
    producer_id: String,
    issuer_id: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

#[derive(Copy, Clone)]
enum EvidenceError {
    Unavailable,
    Oversized,
    Malformed,
    UnsupportedSchema,
    InvalidIntegrity,
    UnauthorizedIssuer,
    UnauthorizedProducer,
    InvalidProof,
    DuplicateCoordinate,
    Uncorrelated,
}

impl EvidenceError {
    const fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Oversized => "oversized",
            Self::Malformed => "malformed",
            Self::UnsupportedSchema => "unsupported_schema",
            Self::InvalidIntegrity => "invalid_integrity",
            Self::UnauthorizedIssuer => "unauthorized_issuer",
            Self::UnauthorizedProducer => "unauthorized_producer",
            Self::InvalidProof => "invalid_proof",
            Self::DuplicateCoordinate => "duplicate_coordinate",
            Self::Uncorrelated => "uncorrelated",
        }
    }
}

impl<C: HttpClient> ForgejoForge<C> {
    /// Acquires evidence only after the caller has matched the authoritative run
    /// and validated every row returned by that run's jobs endpoint. Every
    /// source failure is conservative: the already mapped provider conclusions
    /// remain unchanged and only a secret-free diagnostic code is logged.
    pub(super) async fn apply_verified_failure_evidence(
        &self,
        repo_id: &RepositoryId,
        run_id: u64,
        jobs: &mut [(CiJob, ActionJobDto)],
    ) {
        let Some(config) = self.config().failure_evidence.as_ref() else {
            return;
        };
        if !jobs.iter().any(|(job, _)| eligible_bare_failure(job)) {
            return;
        }
        let proofs = match self.acquire_failure_proofs(config, repo_id, run_id).await {
            Ok(proofs) => proofs,
            Err(error) => {
                tracing::warn!(
                    target: "temper_forge_forgejo",
                    diagnostic = error.code(),
                    "verified CI failure evidence was ignored"
                );
                return;
            }
        };
        let now = Utc::now();
        if proofs.iter().any(|proof| {
            jobs.iter()
                .filter(|(job, provider)| {
                    proof.matches_job_at(
                        job,
                        &provider.id.to_string(),
                        Some(&provider.task_id.to_string()),
                        now,
                    )
                })
                .take(2)
                .count()
                != 1
        }) {
            tracing::warn!(
                target: "temper_forge_forgejo",
                diagnostic = EvidenceError::Uncorrelated.code(),
                "verified CI failure evidence was ignored"
            );
            return;
        }
        for (job, provider) in jobs {
            if !eligible_bare_failure(job) {
                continue;
            }
            let provider_job_id = provider.id.to_string();
            let provider_task_id = provider.task_id.to_string();
            let mut matching = proofs.iter().filter(|proof| {
                proof.matches_job_at(job, &provider_job_id, Some(&provider_task_id), now)
            });
            let Some(proof) = matching.next() else {
                continue;
            };
            if matching.next().is_some() {
                continue;
            }
            job.conclusion = Some(CiJobConclusion::Failure);
            job.verified_failure = Some(proof.clone());
        }
    }

    async fn acquire_failure_proofs(
        &self,
        config: &ForgejoFailureEvidenceConfig,
        repo_id: &RepositoryId,
        run_id: u64,
    ) -> Result<Vec<CiVerifiedFailureProof>, EvidenceError> {
        let request = HttpRequest {
            method: HttpMethod::Get,
            path: config.endpoint().to_string(),
            query: vec![
                ("repository_id".to_string(), repo_id.as_str().to_string()),
                ("run_id".to_string(), run_id.to_string()),
            ],
            headers: vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {}", config.bearer_token()),
                ),
                ("Accept".to_string(), "application/json".to_string()),
            ],
            body: None,
        };
        self.record_provider_request(&request);
        let response = self
            .http_client()
            .execute(request)
            .await
            .map_err(|_| EvidenceError::Unavailable)?;
        if !response.is_success() {
            return Err(EvidenceError::Unavailable);
        }
        if response.body.len() > MAX_RESPONSE_BYTES {
            return Err(EvidenceError::Oversized);
        }
        let response: EvidenceResponse =
            serde_json::from_str(&response.body).map_err(|_| EvidenceError::Malformed)?;
        if response.schema_version != EVIDENCE_SCHEMA_VERSION {
            return Err(EvidenceError::UnsupportedSchema);
        }
        if response.records.len() > MAX_RECORDS {
            return Err(EvidenceError::Oversized);
        }

        let mut seen_coordinates = BTreeSet::new();
        let mut proofs = Vec::with_capacity(response.records.len());
        for record in response.records {
            if record.statement.len() > MAX_STATEMENT_BYTES {
                return Err(EvidenceError::Oversized);
            }
            verify_hmac(
                config.hmac_key(),
                record.statement.as_bytes(),
                &record.hmac_sha256,
            )?;
            let statement: FailureStatement =
                serde_json::from_str(&record.statement).map_err(|_| EvidenceError::Malformed)?;
            if statement.schema_version != EVIDENCE_SCHEMA_VERSION {
                return Err(EvidenceError::UnsupportedSchema);
            }
            if statement.issuer_id != config.issuer_id() {
                return Err(EvidenceError::UnauthorizedIssuer);
            }
            if !config.authorizes_producer(&statement.producer_id) {
                return Err(EvidenceError::UnauthorizedProducer);
            }
            let coordinate = (
                statement.run_id.clone(),
                statement.job_id.clone(),
                statement.attempt.clone(),
                statement.task_id.clone(),
            );
            if !seen_coordinates.insert(coordinate) {
                return Err(EvidenceError::DuplicateCoordinate);
            }
            let subject = CiFailureProofSubject::new(
                statement.repository_id,
                statement.pull_request_id,
                statement.commit_sha,
            )
            .map_err(|_| EvidenceError::InvalidProof)?;
            let coordinates = CiFailureProofCoordinates::new(
                statement.run_id,
                statement.job_id,
                statement.attempt,
                Some(statement.task_id),
            )
            .map_err(|_| EvidenceError::InvalidProof)?;
            let attestation = CiFailureProofAttestation::new(
                statement.producer_id,
                statement.issuer_id,
                CiFailureProofVerification::ProtectedProducer,
            )
            .map_err(|_| EvidenceError::InvalidProof)?;
            proofs.push(
                CiVerifiedFailureProof::new(
                    statement.category,
                    subject,
                    coordinates,
                    attestation,
                    statement.created_at,
                    statement.expires_at,
                )
                .map_err(|_| EvidenceError::InvalidProof)?,
            );
        }
        Ok(proofs)
    }
}

fn eligible_bare_failure(job: &CiJob) -> bool {
    job.status == CiJobStatus::Completed
        && job.conclusion == Some(CiJobConclusion::Unknown)
        && job
            .provider_conclusion
            .as_deref()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("failure"))
        && job.verified_failure.is_none()
}

fn verify_hmac(secret: &str, statement: &[u8], signature: &str) -> Result<(), EvidenceError> {
    let signature = signature.strip_prefix("sha256=").unwrap_or(signature);
    if signature.len() != 64 {
        return Err(EvidenceError::InvalidIntegrity);
    }
    let supplied = decode_hex(signature).ok_or(EvidenceError::InvalidIntegrity)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| EvidenceError::InvalidIntegrity)?;
    mac.update(statement);
    mac.verify_slice(&supplied)
        .map_err(|_| EvidenceError::InvalidIntegrity)
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}
