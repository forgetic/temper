use super::{CiJob, CiJobStatus};
use crate::ids::{PullRequestId, RepositoryId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;

/// Current schema version for a verified ordinary CI-failure proof.
pub const CI_VERIFIED_FAILURE_PROOF_SCHEMA_VERSION: u16 = 1;

/// Maximum UTF-8 byte length of every opaque identity retained in a failure proof.
pub const MAX_CI_FAILURE_PROOF_IDENTITY_BYTES: usize = 128;

/// Maximum interval for which a verified ordinary-failure proof can be valid.
pub const MAX_CI_FAILURE_PROOF_VALIDITY_SECONDS: i64 = 15 * 60;

/// Ordinary, code-repairable category asserted by a verified CI-failure proof.
///
/// This vocabulary deliberately cannot represent runner, startup, timeout,
/// cancellation, or infrastructure failures. Those terminal states must retain
/// their non-ordinary [`CiJobConclusion`](super::CiJobConclusion) instead.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiOrdinaryFailureCategory {
    Source,
    Build,
    Test,
}

/// Typed provenance retained after proof verification.
///
/// `ProtectedProducer` records that the backend authenticated the configured
/// issuer and verified integrity of a record emitted by an allowlisted,
/// protected producer. The credential, signature, and source record are never
/// retained in the portable model.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiFailureProofVerification {
    ProtectedProducer,
}

/// Validation failure for a verified ordinary CI-failure proof.
///
/// Diagnostics identify only the invalid field and bound. They never echo a
/// rejected value, which keeps credentials or other accidental input out of
/// logs and serialized errors.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CiVerifiedFailureProofError {
    #[error("verified CI failure proof field {0} must not be empty")]
    EmptyIdentity(&'static str),
    #[error("verified CI failure proof field {field} exceeds the {maximum}-byte identity bound")]
    IdentityTooLong { field: &'static str, maximum: usize },
    #[error("verified CI failure proof field {0} contains unsupported identity characters")]
    InvalidIdentity(&'static str),
    #[error("verified CI failure proof commit SHA must be an exact 40- or 64-digit hex object id")]
    InvalidCommitSha,
    #[error("verified CI failure proof expiry must be later than creation")]
    InvalidValidityWindow,
    #[error("verified CI failure proof validity exceeds the configured maximum")]
    ValidityTooLong,
}

/// Typed repository, optional pull request, and exact commit claimed by a proof.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CiFailureProofSubject {
    repo_id: RepositoryId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pull_request_id: Option<PullRequestId>,
    commit_sha: String,
}

impl CiFailureProofSubject {
    pub fn new(
        repo_id: RepositoryId,
        pull_request_id: Option<PullRequestId>,
        commit_sha: impl Into<String>,
    ) -> Result<Self, CiVerifiedFailureProofError> {
        let repo_id =
            RepositoryId::new(sanitize_proof_identity("repository id", repo_id.as_str())?);
        let pull_request_id = pull_request_id
            .map(|id| {
                sanitize_proof_identity("pull request id", id.as_str()).map(PullRequestId::new)
            })
            .transpose()?;
        let commit_sha = sanitize_exact_commit_sha(&commit_sha.into())?;
        Ok(Self {
            repo_id,
            pull_request_id,
            commit_sha,
        })
    }

    pub fn repo_id(&self) -> &RepositoryId {
        &self.repo_id
    }

    pub fn pull_request_id(&self) -> Option<&PullRequestId> {
        self.pull_request_id.as_ref()
    }

    pub fn commit_sha(&self) -> &str {
        &self.commit_sha
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CiFailureProofSubjectWire {
    repo_id: RepositoryId,
    pull_request_id: Option<PullRequestId>,
    commit_sha: String,
}

impl<'de> Deserialize<'de> for CiFailureProofSubject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CiFailureProofSubjectWire::deserialize(deserializer)?;
        Self::new(wire.repo_id, wire.pull_request_id, wire.commit_sha).map_err(de::Error::custom)
    }
}

/// Exact provider execution coordinate asserted by a proof.
///
/// Run, job, and attempt are always required. `task_id` is optional for
/// providers without that extra coordinate and required by an integrating
/// backend whenever the provider exposes one (including Forgejo Actions).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CiFailureProofCoordinates {
    run_id: String,
    job_id: String,
    attempt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
}

impl CiFailureProofCoordinates {
    pub fn new(
        run_id: impl Into<String>,
        job_id: impl Into<String>,
        attempt: impl Into<String>,
        task_id: Option<impl Into<String>>,
    ) -> Result<Self, CiVerifiedFailureProofError> {
        Ok(Self {
            run_id: sanitize_proof_identity("run id", &run_id.into())?,
            job_id: sanitize_proof_identity("job id", &job_id.into())?,
            attempt: sanitize_proof_identity("attempt", &attempt.into())?,
            task_id: task_id
                .map(|value| sanitize_proof_identity("task id", &value.into()))
                .transpose()?,
        })
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn attempt(&self) -> &str {
        &self.attempt
    }

    pub fn task_id(&self) -> Option<&str> {
        self.task_id.as_deref()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CiFailureProofCoordinatesWire {
    run_id: String,
    job_id: String,
    attempt: String,
    task_id: Option<String>,
}

impl<'de> Deserialize<'de> for CiFailureProofCoordinates {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CiFailureProofCoordinatesWire::deserialize(deserializer)?;
        Self::new(wire.run_id, wire.job_id, wire.attempt, wire.task_id).map_err(de::Error::custom)
    }
}

/// Bounded producer/issuer identities and the retained verification result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CiFailureProofAttestation {
    producer_id: String,
    issuer_id: String,
    verification: CiFailureProofVerification,
}

impl CiFailureProofAttestation {
    pub fn new(
        producer_id: impl Into<String>,
        issuer_id: impl Into<String>,
        verification: CiFailureProofVerification,
    ) -> Result<Self, CiVerifiedFailureProofError> {
        Ok(Self {
            producer_id: sanitize_proof_identity("producer id", &producer_id.into())?,
            issuer_id: sanitize_proof_identity("issuer id", &issuer_id.into())?,
            verification,
        })
    }

    pub fn producer_id(&self) -> &str {
        &self.producer_id
    }

    pub fn issuer_id(&self) -> &str {
        &self.issuer_id
    }

    pub const fn verification(&self) -> CiFailureProofVerification {
        self.verification
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CiFailureProofAttestationWire {
    producer_id: String,
    issuer_id: String,
    verification: CiFailureProofVerification,
}

impl<'de> Deserialize<'de> for CiFailureProofAttestation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CiFailureProofAttestationWire::deserialize(deserializer)?;
        Self::new(wire.producer_id, wire.issuer_id, wire.verification).map_err(de::Error::custom)
    }
}

/// Provider-neutral, verified proof of an ordinary source/build/test failure.
///
/// The value is intentionally an integrity-free diagnostic record: a backend
/// must authenticate and verify the protected producer before constructing it.
/// Deserializing a value does not verify it and never changes a job conclusion.
/// Signatures, credentials, logs, UI text, workflow/repository names, scenario
/// identity, and free-form descriptions have no field in this schema.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CiVerifiedFailureProof {
    schema_version: u16,
    category: CiOrdinaryFailureCategory,
    subject: CiFailureProofSubject,
    coordinates: CiFailureProofCoordinates,
    attestation: CiFailureProofAttestation,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

impl CiVerifiedFailureProof {
    pub fn new(
        category: CiOrdinaryFailureCategory,
        subject: CiFailureProofSubject,
        coordinates: CiFailureProofCoordinates,
        attestation: CiFailureProofAttestation,
        created_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<Self, CiVerifiedFailureProofError> {
        if expires_at <= created_at {
            return Err(CiVerifiedFailureProofError::InvalidValidityWindow);
        }
        if expires_at.signed_duration_since(created_at)
            > chrono::Duration::seconds(MAX_CI_FAILURE_PROOF_VALIDITY_SECONDS)
        {
            return Err(CiVerifiedFailureProofError::ValidityTooLong);
        }
        Ok(Self {
            schema_version: CI_VERIFIED_FAILURE_PROOF_SCHEMA_VERSION,
            category,
            subject,
            coordinates,
            attestation,
            created_at,
            expires_at,
        })
    }

    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub const fn category(&self) -> CiOrdinaryFailureCategory {
        self.category
    }

    pub fn subject(&self) -> &CiFailureProofSubject {
        &self.subject
    }

    pub fn coordinates(&self) -> &CiFailureProofCoordinates {
        &self.coordinates
    }

    pub fn attestation(&self) -> &CiFailureProofAttestation {
        &self.attestation
    }

    pub const fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    pub const fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// Whether the proof is valid at `now`; expiry is an exclusive boundary.
    pub fn is_fresh_at(&self, now: DateTime<Utc>) -> bool {
        now >= self.created_at && now < self.expires_at
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CiVerifiedFailureProofWire {
    schema_version: u16,
    category: CiOrdinaryFailureCategory,
    subject: CiFailureProofSubject,
    coordinates: CiFailureProofCoordinates,
    attestation: CiFailureProofAttestation,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

impl<'de> Deserialize<'de> for CiVerifiedFailureProof {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CiVerifiedFailureProofWire::deserialize(deserializer)?;
        if wire.schema_version != CI_VERIFIED_FAILURE_PROOF_SCHEMA_VERSION {
            return Err(de::Error::custom(
                "unsupported verified CI failure proof schema version",
            ));
        }
        Self::new(
            wire.category,
            wire.subject,
            wire.coordinates,
            wire.attestation,
            wire.created_at,
            wire.expires_at,
        )
        .map_err(de::Error::custom)
    }
}

fn sanitize_proof_identity(
    field: &'static str,
    value: &str,
) -> Result<String, CiVerifiedFailureProofError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CiVerifiedFailureProofError::EmptyIdentity(field));
    }
    if value.len() > MAX_CI_FAILURE_PROOF_IDENTITY_BYTES {
        return Err(CiVerifiedFailureProofError::IdentityTooLong {
            field,
            maximum: MAX_CI_FAILURE_PROOF_IDENTITY_BYTES,
        });
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
    }) {
        return Err(CiVerifiedFailureProofError::InvalidIdentity(field));
    }
    Ok(value.to_string())
}

fn sanitize_exact_commit_sha(value: &str) -> Result<String, CiVerifiedFailureProofError> {
    let value = value.trim();
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CiVerifiedFailureProofError::InvalidCommitSha);
    }
    Ok(value.to_ascii_lowercase())
}

impl CiVerifiedFailureProof {
    /// Checks every portable and provider execution coordinate for one job.
    ///
    /// The caller supplies the raw provider job/task coordinates because a
    /// portable [`CiJobId`](crate::ids::CiJobId) is backend-encoded and must not be parsed by model
    /// consumers. Providers with task identities must pass `Some(task_id)`.
    pub fn matches_job_at(
        &self,
        job: &CiJob,
        provider_job_id: &str,
        provider_task_id: Option<&str>,
        now: DateTime<Utc>,
    ) -> bool {
        job.status == CiJobStatus::Completed
            && job.repo_id == self.subject.repo_id
            && job.pull_request_id.as_ref() == self.subject.pull_request_id.as_ref()
            && job
                .commit_sha
                .eq_ignore_ascii_case(&self.subject.commit_sha)
            && job.run_id.as_deref() == Some(self.coordinates.run_id.as_str())
            && job.attempt.as_deref() == Some(self.coordinates.attempt.as_str())
            && provider_job_id == self.coordinates.job_id
            && provider_task_id == self.coordinates.task_id.as_deref()
            && self.is_fresh_at(now)
    }
}

#[cfg(test)]
mod tests;
