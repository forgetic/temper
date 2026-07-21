// SPDX-License-Identifier: MPL-2.0

//! Exact-attempt cancellation directives returned by the daemon.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::WORKER_PROTOCOL_VERSION;

/// Maximum UTF-8 encoded length of a stable cancellation reason.
pub const MAX_ATTEMPT_CANCELLATION_REASON_BYTES: usize = 512;

const MAX_CANCELLATION_ID_BYTES: usize = 256;

/// Stable cause vocabulary for daemon-requested attempt cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptCancellationCause {
    OwnershipLost,
}

/// One exact worker/job/attempt identity that must stop running.
///
/// `attempt_id` is optional only when reading legacy metadata. New directives
/// can be built only with [`AttemptCancellation::ownership_lost`], which
/// requires a non-blank attempt id.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttemptCancellation {
    worker_id: String,
    job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempt_id: Option<String>,
    cause: AttemptCancellationCause,
    reason: String,
}

impl AttemptCancellation {
    /// Builds a modern ownership-loss directive for one fenced attempt.
    pub fn ownership_lost(
        worker_id: impl Into<String>,
        job_id: impl Into<String>,
        attempt_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<Self, CancelAttemptsError> {
        let cancellation = Self {
            worker_id: worker_id.into(),
            job_id: job_id.into(),
            attempt_id: Some(attempt_id.into()),
            cause: AttemptCancellationCause::OwnershipLost,
            reason: reason.into(),
        };
        validate_cancellation(&cancellation, true)?;
        Ok(cancellation)
    }

    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn attempt_id(&self) -> Option<&str> {
        self.attempt_id.as_deref()
    }

    pub fn cause(&self) -> AttemptCancellationCause {
        self.cause
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Exact identity comparison. A legacy `None` attempt matches only another
    /// `None`; it never acts as a wildcard for a modern fenced attempt.
    pub fn matches_exact(&self, worker_id: &str, job_id: &str, attempt_id: Option<&str>) -> bool {
        self.worker_id == worker_id
            && self.job_id == job_id
            && self.attempt_id.as_deref() == attempt_id
    }
}

/// One daemon response to a worker heartbeat, carrying one or more exact
/// cancellation directives.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelAttempts {
    protocol_version: u32,
    worker_id: String,
    cancellations: Vec<AttemptCancellation>,
}

impl CancelAttempts {
    /// Builds a modern v1 directive. Entries are validated and normalized into
    /// deterministic `(job_id, attempt_id)` order.
    pub fn new(
        worker_id: impl Into<String>,
        cancellations: Vec<AttemptCancellation>,
    ) -> Result<Self, CancelAttemptsError> {
        let mut directive = Self {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.into(),
            cancellations,
        };
        directive.validate(true)?;
        directive.sort_cancellations();
        Ok(directive)
    }

    pub fn protocol_version(&self) -> u32 {
        self.protocol_version
    }

    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    pub fn cancellations(&self) -> &[AttemptCancellation] {
        &self.cancellations
    }

    fn validate(&self, require_attempt_id: bool) -> Result<(), CancelAttemptsError> {
        validate_id("worker_id", &self.worker_id)?;
        if self.cancellations.is_empty() {
            return Err(CancelAttemptsError::Empty);
        }

        let mut identities = BTreeSet::new();
        for cancellation in &self.cancellations {
            validate_cancellation(cancellation, require_attempt_id)?;
            if cancellation.worker_id != self.worker_id {
                return Err(CancelAttemptsError::WorkerMismatch);
            }
            let identity = (cancellation.job_id.clone(), cancellation.attempt_id.clone());
            if !identities.insert(identity) {
                return Err(CancelAttemptsError::DuplicateIdentity);
            }
        }
        Ok(())
    }

    fn sort_cancellations(&mut self) {
        self.cancellations.sort_by(|left, right| {
            (&left.job_id, &left.attempt_id).cmp(&(&right.job_id, &right.attempt_id))
        });
    }
}

#[derive(Serialize, Deserialize)]
struct CancelAttemptsWire {
    protocol_version: u32,
    worker_id: String,
    cancellations: Vec<AttemptCancellation>,
}

impl Serialize for CancelAttempts {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate(false).map_err(serde::ser::Error::custom)?;
        let mut cancellations = self.cancellations.clone();
        cancellations.sort_by(|left, right| {
            (&left.job_id, &left.attempt_id).cmp(&(&right.job_id, &right.attempt_id))
        });
        CancelAttemptsWire {
            protocol_version: self.protocol_version,
            worker_id: self.worker_id.clone(),
            cancellations,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CancelAttempts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CancelAttemptsWire::deserialize(deserializer)?;
        let mut directive = Self {
            protocol_version: wire.protocol_version,
            worker_id: wire.worker_id,
            cancellations: wire.cancellations,
        };
        // Legacy metadata may omit attempt_id. Every identity check remains an
        // exact Option comparison, so omission cannot cancel a modern attempt.
        directive.validate(false).map_err(D::Error::custom)?;
        directive.sort_cancellations();
        Ok(directive)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CancelAttemptsError {
    #[error("cancel-attempts must contain at least one cancellation")]
    Empty,
    #[error("cancellation entry worker_id does not match the envelope worker_id")]
    WorkerMismatch,
    #[error("cancel-attempts contains a duplicate exact attempt identity")]
    DuplicateIdentity,
    #[error("{0} must be non-blank and at most 256 bytes")]
    InvalidId(&'static str),
    #[error("modern cancellation directives require a non-blank attempt_id")]
    MissingAttemptId,
    #[error("cancellation reason must be non-blank and at most 512 bytes")]
    InvalidReason,
}

fn validate_cancellation(
    cancellation: &AttemptCancellation,
    require_attempt_id: bool,
) -> Result<(), CancelAttemptsError> {
    validate_id("worker_id", &cancellation.worker_id)?;
    validate_id("job_id", &cancellation.job_id)?;
    match cancellation.attempt_id.as_deref() {
        Some(attempt_id) => validate_id("attempt_id", attempt_id)?,
        None if require_attempt_id => return Err(CancelAttemptsError::MissingAttemptId),
        None => {}
    }
    if cancellation.reason.trim().is_empty()
        || cancellation.reason.len() > MAX_ATTEMPT_CANCELLATION_REASON_BYTES
    {
        return Err(CancelAttemptsError::InvalidReason);
    }
    Ok(())
}

fn validate_id(field: &'static str, value: &str) -> Result<(), CancelAttemptsError> {
    if value.trim().is_empty() || value.len() > MAX_CANCELLATION_ID_BYTES {
        Err(CancelAttemptsError::InvalidId(field))
    } else {
        Ok(())
    }
}
