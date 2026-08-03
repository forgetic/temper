use crate::ids::{CiJobId, PullRequestId, RepositoryId};
use crate::model::{CiJob, CiJobConclusion, CiJobStatus, CiVerifiedFailureProof};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

/// Exact portable snapshot of one job in the latest provider attempt.
///
/// The snapshot deliberately includes both provider-neutral state and bounded
/// provider evidence. A retry backend reconstructs this set immediately before
/// issuing a provider mutation; any change fails the fence rather than widening
/// the request to a different job set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRetryJobFingerprint {
    pub id: CiJobId,
    pub name: String,
    pub status: CiJobStatus,
    pub conclusion: Option<CiJobConclusion>,
    pub provider_conclusion: Option<String>,
    pub provider_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_failure: Option<CiVerifiedFailureProof>,
    pub updated_at: DateTime<Utc>,
}

/// Deterministically ordered fingerprint of the authoritative latest job set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRetryJobSetFingerprint {
    jobs: Vec<CiRetryJobFingerprint>,
}

impl CiRetryJobSetFingerprint {
    /// Builds a fingerprint, rejecting an empty set or duplicate stable job ids.
    pub fn from_jobs(jobs: &[CiJob]) -> Result<Self, CiRetryRequestError> {
        if jobs.is_empty() {
            return Err(CiRetryRequestError::EmptyJobSet);
        }
        let mut ids = BTreeSet::new();
        let mut fingerprint = Vec::with_capacity(jobs.len());
        for job in jobs {
            if !ids.insert(job.id.clone()) {
                return Err(CiRetryRequestError::DuplicateJob(job.id.clone()));
            }
            fingerprint.push(CiRetryJobFingerprint {
                id: job.id.clone(),
                name: job.name.clone(),
                status: job.status,
                conclusion: job.conclusion,
                provider_conclusion: job.provider_conclusion.clone(),
                provider_reason: job.provider_reason.clone(),
                verified_failure: job.verified_failure.clone(),
                updated_at: job.updated_at,
            });
        }
        fingerprint.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(Self { jobs: fingerprint })
    }

    pub fn jobs(&self) -> &[CiRetryJobFingerprint] {
        &self.jobs
    }
}

/// Construction errors for an exact-attempt CI retry request.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CiRetryRequestError {
    #[error("CI retry {0} must not be empty")]
    EmptyIdentity(&'static str),
    #[error("CI retry requires at least one authoritative latest-attempt job")]
    EmptyJobSet,
    #[error("CI retry latest-attempt jobs contain duplicate id {0}")]
    DuplicateJob(CiJobId),
    #[error("CI retry job {job} is outside the requested {field} fence")]
    JobFenceMismatch { job: CiJobId, field: &'static str },
}

/// Provider-neutral request to retry exactly one repository/PR/head/run attempt.
///
/// Callers must construct this from a freshly read authoritative latest job set.
/// Every backend revalidates all coordinates and the fingerprint before any
/// provider mutation. Fields are private so ordinary Rust callers cannot omit a
/// fence; deserialized values receive the same backend validation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiRetryRequest {
    repo_id: RepositoryId,
    pull_request_id: PullRequestId,
    head_sha: String,
    run_id: String,
    attempt: String,
    latest_jobs: CiRetryJobSetFingerprint,
}

impl CiRetryRequest {
    pub fn new(
        repo_id: RepositoryId,
        pull_request_id: PullRequestId,
        head_sha: impl Into<String>,
        run_id: impl Into<String>,
        attempt: impl Into<String>,
        jobs: &[CiJob],
    ) -> Result<Self, CiRetryRequestError> {
        let head_sha = nonempty("head SHA", head_sha.into())?;
        let run_id = nonempty("run id", run_id.into())?;
        let attempt = nonempty("attempt", attempt.into())?;
        for job in jobs {
            let mismatch = if job.repo_id != repo_id {
                Some("repository")
            } else if job.pull_request_id.as_ref() != Some(&pull_request_id) {
                Some("pull request")
            } else if job.commit_sha != head_sha {
                Some("head SHA")
            } else if job.run_id.as_deref() != Some(run_id.as_str()) {
                Some("run")
            } else if job.attempt.as_deref() != Some(attempt.as_str()) {
                Some("attempt")
            } else {
                None
            };
            if let Some(field) = mismatch {
                return Err(CiRetryRequestError::JobFenceMismatch {
                    job: job.id.clone(),
                    field,
                });
            }
        }
        Ok(Self {
            repo_id,
            pull_request_id,
            head_sha,
            run_id,
            attempt,
            latest_jobs: CiRetryJobSetFingerprint::from_jobs(jobs)?,
        })
    }

    pub fn repo_id(&self) -> &RepositoryId {
        &self.repo_id
    }

    pub fn pull_request_id(&self) -> &PullRequestId {
        &self.pull_request_id
    }

    pub fn head_sha(&self) -> &str {
        &self.head_sha
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn attempt(&self) -> &str {
        &self.attempt
    }

    pub fn latest_jobs(&self) -> &CiRetryJobSetFingerprint {
        &self.latest_jobs
    }

    /// Checks freshly read jobs against every request coordinate and snapshot.
    pub fn matches_jobs(&self, jobs: &[CiJob]) -> bool {
        jobs.iter().all(|job| {
            job.repo_id == self.repo_id
                && job.pull_request_id.as_ref() == Some(&self.pull_request_id)
                && job.commit_sha == self.head_sha
                && job.run_id.as_deref() == Some(self.run_id.as_str())
                && job.attempt.as_deref() == Some(self.attempt.as_str())
        }) && CiRetryJobSetFingerprint::from_jobs(jobs)
            .is_ok_and(|fingerprint| fingerprint == self.latest_jobs)
    }
}

fn nonempty(field: &'static str, value: String) -> Result<String, CiRetryRequestError> {
    let value = value.trim().to_string();
    if value.is_empty() {
        Err(CiRetryRequestError::EmptyIdentity(field))
    } else {
        Ok(value)
    }
}

/// Why an exact retry request was rejected before the provider mutation.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiRetryRejection {
    RepositoryMismatch,
    PullRequestMismatch,
    HeadChanged,
    RunChanged,
    AttemptChanged,
    JobSetChanged,
    RunNotRetryable,
    ProviderRejected,
}

/// Explicit result of an exact-attempt CI retry request.
///
/// `Uncertain` means the backend cannot prove whether the provider accepted the
/// operation. Callers must reconcile it with a later authoritative CI read and
/// must not issue an unbounded duplicate. `AlreadyObserved` means a prior retry
/// (or a newer attempt) is visible and no mutation was issued.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiRetryOutcome {
    Accepted,
    AlreadyObserved,
    Unsupported,
    Rejected(CiRetryRejection),
    Uncertain,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CiJobId, PullRequestId, RepositoryId};

    fn job(id: &str) -> CiJob {
        CiJob {
            id: CiJobId::new(id),
            repo_id: RepositoryId::new("repo-1"),
            pull_request_id: Some(PullRequestId::new("pr-1")),
            commit_sha: "head-1".into(),
            name: id.into(),
            status: CiJobStatus::Completed,
            conclusion: Some(CiJobConclusion::RunnerLost),
            provider_conclusion: Some("runner_lost".into()),
            provider_reason: None,
            run_id: Some("run-7".into()),
            attempt: Some("2".into()),
            verified_failure: None,
            url: None,
            created_at: DateTime::UNIX_EPOCH,
            started_at: None,
            completed_at: Some(DateTime::UNIX_EPOCH),
            updated_at: DateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn request_fences_every_coordinate_and_sorts_the_job_set() {
        let jobs = vec![job("job-b"), job("job-a")];
        let request = CiRetryRequest::new(
            RepositoryId::new("repo-1"),
            PullRequestId::new("pr-1"),
            "head-1",
            "run-7",
            "2",
            &jobs,
        )
        .unwrap();
        assert_eq!(request.latest_jobs().jobs()[0].id, CiJobId::new("job-a"));
        assert!(request.matches_jobs(&jobs));

        let mut foreign = jobs.clone();
        foreign[0].commit_sha = "another-head".into();
        assert!(matches!(
            CiRetryRequest::new(
                RepositoryId::new("repo-1"),
                PullRequestId::new("pr-1"),
                "head-1",
                "run-7",
                "2",
                &foreign,
            ),
            Err(CiRetryRequestError::JobFenceMismatch {
                field: "head SHA",
                ..
            })
        ));
        assert!(!request.matches_jobs(&foreign));
    }

    #[test]
    fn request_rejects_empty_and_duplicate_job_sets() {
        assert!(matches!(
            CiRetryRequest::new(
                RepositoryId::new("repo-1"),
                PullRequestId::new("pr-1"),
                "head-1",
                "run-7",
                "2",
                &[],
            ),
            Err(CiRetryRequestError::EmptyJobSet)
        ));
        let duplicate = vec![job("same"), job("same")];
        assert!(matches!(
            CiRetryRequest::new(
                RepositoryId::new("repo-1"),
                PullRequestId::new("pr-1"),
                "head-1",
                "run-7",
                "2",
                &duplicate,
            ),
            Err(CiRetryRequestError::DuplicateJob(_))
        ));
    }
}
