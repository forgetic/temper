use temper_agent_core::{
    ModelCallStatus, ModelFailureCategory, ModelFailureDiagnostic, ModelIdentity,
};
use temper_protocol_activity::{
    FailureCodeV1, ModelCallStatusV1, ModelFailureCategoryV1, ModelFailureV1, StopReasonV1,
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
        ModelCallStatus::Failed => Some(failure.map_or_else(
            || ModelFailureV1::redacted_unknown(model.provider.clone(), model.model.clone(), false),
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

fn map_diagnostic(diagnostic: ModelFailureDiagnostic) -> ModelFailureV1 {
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
