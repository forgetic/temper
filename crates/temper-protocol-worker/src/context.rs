// SPDX-License-Identifier: MPL-2.0

//! Authenticated worker requests for bounded, read-only Forge context.

use serde::{Deserialize, Serialize};

use crate::{
    ForgeContextErrorCode, ForgeContextOperation, ForgeContextResult, WORKER_PROTOCOL_VERSION,
};

/// One worker request to read bounded context for its active assignment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FetchContext {
    pub protocol_version: u32,
    pub worker_id: String,
    pub job_id: String,
    /// Exact assignment fence. Optional only when reading legacy requests;
    /// current workers always send the id copied from `Assign`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    pub operation: ForgeContextOperation,
}

impl FetchContext {
    pub fn new(
        worker_id: impl Into<String>,
        job_id: impl Into<String>,
        attempt_id: impl Into<String>,
        operation: ForgeContextOperation,
    ) -> Self {
        Self {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.into(),
            job_id: job_id.into(),
            attempt_id: Some(attempt_id.into()),
            operation,
        }
    }
}

/// Exactly one successful bounded result or one stable public error code.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextResponse {
    pub protocol_version: u32,
    pub worker_id: String,
    pub job_id: String,
    #[serde(flatten)]
    pub outcome: ContextOutcome,
}

impl ContextResponse {
    pub fn success(request: &FetchContext, result: ForgeContextResult) -> Self {
        Self {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: request.worker_id.clone(),
            job_id: request.job_id.clone(),
            outcome: ContextOutcome::Success { result },
        }
    }

    pub fn error(request: &FetchContext, code: ForgeContextErrorCode) -> Self {
        Self {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: request.worker_id.clone(),
            job_id: request.job_id.clone(),
            outcome: ContextOutcome::Error { code },
        }
    }
}

/// Tagged response outcome prevents ambiguous result/error combinations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ContextOutcome {
    Success { result: ForgeContextResult },
    Error { code: ForgeContextErrorCode },
}
