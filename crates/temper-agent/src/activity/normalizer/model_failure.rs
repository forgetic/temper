use temper_agent_core::{
    ModelCallStatus, ModelFailureBoundary, ModelFailureCategory, ModelFailureDiagnostic,
    ModelFailureDisposition, ModelFailureEventKind, ModelIdentity,
};
use temper_protocol_activity::{
    FailureCodeV1, ModelCallStatusV1, ModelFailureBoundaryV1, ModelFailureCategoryV1,
    ModelFailureDispositionV1, ModelFailureEventKindV1, ModelFailureV1, StopReasonV1,
};

pub(super) fn normalize_finish(
    model: &ModelIdentity,
    status: ModelCallStatus,
    stop_reason: Option<StopReasonV1>,
    failure: Option<ModelFailureDiagnostic>,
) -> (ModelCallStatus, Option<ModelFailureV1>) {
    let status = if status == ModelCallStatus::Succeeded && stop_reason == Some(StopReasonV1::Error)
    {
        ModelCallStatus::Failed
    } else {
        status
    };
    let failure = match status {
        ModelCallStatus::Failed => Some(failure.as_ref().map_or_else(
            || {
                ModelFailureV1::unknown(
                    model.provider.clone(),
                    model.model.clone(),
                    ModelFailureBoundaryV1::Local,
                    ModelFailureEventKindV1::LocalError,
                )
            },
            map_diagnostic,
        )),
        ModelCallStatus::Succeeded | ModelCallStatus::Cancelled => None,
    };
    (status, failure)
}

pub(super) fn status(status: ModelCallStatus) -> ModelCallStatusV1 {
    match status {
        ModelCallStatus::Succeeded => ModelCallStatusV1::Succeeded,
        ModelCallStatus::Failed => ModelCallStatusV1::Failed,
        ModelCallStatus::Cancelled => ModelCallStatusV1::Cancelled,
    }
}

pub(super) fn retry_code(category: ModelFailureCategory) -> FailureCodeV1 {
    match category {
        ModelFailureCategory::Timeout => FailureCodeV1::Timeout,
        ModelFailureCategory::Transport
        | ModelFailureCategory::RateLimit
        | ModelFailureCategory::Authentication
        | ModelFailureCategory::Context
        | ModelFailureCategory::Response
        | ModelFailureCategory::Provider
        | ModelFailureCategory::RedactedUnknown => FailureCodeV1::Provider,
    }
}

pub(super) fn disposition(value: ModelFailureDisposition) -> ModelFailureDispositionV1 {
    match value {
        ModelFailureDisposition::Retryable => ModelFailureDispositionV1::Retryable,
        ModelFailureDisposition::NonRetryable => ModelFailureDispositionV1::NonRetryable,
        ModelFailureDisposition::Unknown => ModelFailureDispositionV1::Unknown,
    }
}

pub(in crate::activity) fn map_diagnostic(diagnostic: &ModelFailureDiagnostic) -> ModelFailureV1 {
    let mut failure = ModelFailureV1 {
        provider: diagnostic.provider().to_string(),
        model: diagnostic.model().to_string(),
        category: match diagnostic.category() {
            ModelFailureCategory::Timeout => ModelFailureCategoryV1::Timeout,
            ModelFailureCategory::Transport => ModelFailureCategoryV1::Transport,
            ModelFailureCategory::RateLimit => ModelFailureCategoryV1::RateLimit,
            ModelFailureCategory::Authentication => ModelFailureCategoryV1::Authentication,
            ModelFailureCategory::Context => ModelFailureCategoryV1::Context,
            ModelFailureCategory::Response => ModelFailureCategoryV1::Response,
            ModelFailureCategory::Provider => ModelFailureCategoryV1::Provider,
            ModelFailureCategory::RedactedUnknown => ModelFailureCategoryV1::RedactedUnknown,
        },
        disposition: disposition(diagnostic.disposition()),
        boundary: match diagnostic.boundary() {
            ModelFailureBoundary::Http => ModelFailureBoundaryV1::Http,
            ModelFailureBoundary::Sse => ModelFailureBoundaryV1::Sse,
            ModelFailureBoundary::Local => ModelFailureBoundaryV1::Local,
        },
        event_kind: match diagnostic.event_kind() {
            ModelFailureEventKind::HttpResponse => ModelFailureEventKindV1::HttpResponse,
            ModelFailureEventKind::StreamError => ModelFailureEventKindV1::StreamError,
            ModelFailureEventKind::ErrorCompletion => ModelFailureEventKindV1::ErrorCompletion,
            ModelFailureEventKind::StreamEof => ModelFailureEventKindV1::StreamEof,
            ModelFailureEventKind::ConnectTimeout => ModelFailureEventKindV1::ConnectTimeout,
            ModelFailureEventKind::StreamIdleTimeout => ModelFailureEventKindV1::StreamIdleTimeout,
            ModelFailureEventKind::Transport => ModelFailureEventKindV1::Transport,
            ModelFailureEventKind::LocalError => ModelFailureEventKindV1::LocalError,
        },
        status_present: diagnostic.status_present(),
        code_present: diagnostic.code_present(),
        retryable: diagnostic.retryable(),
        http_status: diagnostic.http_status(),
        provider_request_id: diagnostic.provider_request_id().map(str::to_owned),
        provider_error_code: diagnostic.provider_error_code().map(str::to_owned),
        message: diagnostic.message().to_string(),
        detail_redacted: diagnostic.detail_redacted(),
    };
    failure.normalize();
    failure
}
