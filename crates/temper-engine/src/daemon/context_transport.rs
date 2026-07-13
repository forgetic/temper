// SPDX-License-Identifier: MPL-2.0

//! Validation and stable-error handling for worker context-read messages.

use temper_engine_io::http::HttpResponder;
use temper_protocol_worker::{
    ContextOutcome, ContextResponse, FetchContext, ForgeContextErrorCode, ForgeContextOperation,
    WORKER_PROTOCOL_VERSION, WorkerAuth,
};

use super::machine::{ContextReadAudit, DaemonMachine, DaemonRequest};

const MAX_CONTEXT_ID_BYTES: usize = 256;

impl DaemonMachine {
    pub(super) fn handle_fetch_context(
        &self,
        fetch: FetchContext,
        auth: Option<WorkerAuth>,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        if fetch.protocol_version != WORKER_PROTOCOL_VERSION
            || fetch.worker_id.is_empty()
            || fetch.worker_id.len() > MAX_CONTEXT_ID_BYTES
            || fetch.job_id.is_empty()
            || fetch.job_id.len() > MAX_CONTEXT_ID_BYTES
        {
            return self.context_error_requests(
                fetch,
                "unknown",
                ForgeContextErrorCode::InvalidRequest,
                responder,
            );
        }
        let job =
            match self
                .core
                .authorize_context_read(&fetch.worker_id, &fetch.job_id, auth.as_ref())
            {
                Ok(Some(job)) => job,
                Ok(None) | Err(_) => {
                    return self.context_error_requests(
                        fetch,
                        "unknown",
                        ForgeContextErrorCode::NotAuthorized,
                        responder,
                    );
                }
            };
        if let Err(code) = crate::artifact_context::validate_context_operation(
            &fetch.operation,
            &self.artifact_catalog,
        ) {
            return self.context_error_requests(fetch, &job.role, code, responder);
        }
        vec![DaemonRequest::RunFetchContext {
            request: fetch,
            role: job.role,
            responder,
        }]
    }

    pub(super) fn context_error_requests(
        &self,
        fetch: FetchContext,
        role: &str,
        code: ForgeContextErrorCode,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        let audit = context_read_audit(&fetch, role, context_error_name(code));
        vec![DaemonRequest::RespondContext {
            response: ContextResponse::error(&fetch, code),
            audit,
            responder,
        }]
    }
}

pub(super) fn malformed_context_response(
    body: &[u8],
) -> Option<(ContextResponse, ContextReadAudit)> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    if value.get("type")?.as_str()? != "fetch-context" {
        return None;
    }
    let worker_id = value.get("worker_id")?.as_str()?;
    let job_id = value.get("job_id")?.as_str()?;
    if worker_id.is_empty()
        || worker_id.len() > MAX_CONTEXT_ID_BYTES
        || job_id.is_empty()
        || job_id.len() > MAX_CONTEXT_ID_BYTES
    {
        return None;
    }
    let operation = value.get("operation");
    let repository = operation
        .and_then(|operation| operation.get("repo"))
        .and_then(serde_json::Value::as_str)
        .filter(|repository| repository.len() <= 512)
        .unwrap_or("");
    let item_number = operation
        .and_then(|operation| operation.get("number"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let response = ContextResponse {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        outcome: ContextOutcome::Error {
            code: ForgeContextErrorCode::InvalidRequest,
        },
    };
    let audit = ContextReadAudit {
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        role: "unknown".to_string(),
        operation: "invalid".to_string(),
        repository: repository.to_string(),
        item_number,
        status: "invalid_request".to_string(),
    };
    Some((response, audit))
}

fn context_read_audit(fetch: &FetchContext, role: &str, status: &str) -> ContextReadAudit {
    ContextReadAudit {
        worker_id: fetch.worker_id.clone(),
        job_id: fetch.job_id.clone(),
        role: role.to_string(),
        operation: match &fetch.operation {
            ForgeContextOperation::ForgeGetItem(_) => "forge_get_item",
            ForgeContextOperation::ForgeListRelated(_) => "forge_list_related",
        }
        .to_string(),
        repository: fetch.operation.repository().to_string(),
        item_number: fetch.operation.number(),
        status: status.to_string(),
    }
}

fn context_error_name(code: ForgeContextErrorCode) -> &'static str {
    match code {
        ForgeContextErrorCode::InvalidRequest => "invalid_request",
        ForgeContextErrorCode::NotAuthorized => "not_authorized",
        ForgeContextErrorCode::NotFound => "not_found",
        ForgeContextErrorCode::ForgeUnavailable => "forge_unavailable",
        ForgeContextErrorCode::LimitExceeded => "limit_exceeded",
    }
}
