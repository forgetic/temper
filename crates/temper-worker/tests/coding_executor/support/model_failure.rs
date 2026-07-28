use temper_protocol_activity::{
    ModelFailureBoundaryV1, ModelFailureCategoryV1, ModelFailureDispositionV1,
    ModelFailureEventKindV1, ModelFailureV1, REDACTED_MODEL_FAILURE_MESSAGE,
};

pub(super) fn retryable_model_failure() -> ModelFailureV1 {
    fixture(
        ModelFailureCategoryV1::Provider,
        ModelFailureDispositionV1::Retryable,
        503,
        "unavailable",
    )
}

pub(super) fn non_retryable_model_failure() -> ModelFailureV1 {
    fixture(
        ModelFailureCategoryV1::Authentication,
        ModelFailureDispositionV1::NonRetryable,
        401,
        "invalid_api_key",
    )
}

fn fixture(
    category: ModelFailureCategoryV1,
    disposition: ModelFailureDispositionV1,
    status: u16,
    code: &str,
) -> ModelFailureV1 {
    ModelFailureV1 {
        provider: "fixture-provider".into(),
        model: "fixture-model".into(),
        category,
        disposition,
        boundary: ModelFailureBoundaryV1::Http,
        event_kind: ModelFailureEventKindV1::HttpResponse,
        status_present: true,
        code_present: true,
        retryable: disposition == ModelFailureDispositionV1::Retryable,
        http_status: Some(status),
        provider_request_id: Some("fixture-request".into()),
        provider_error_code: Some(code.into()),
        message: REDACTED_MODEL_FAILURE_MESSAGE.into(),
        detail_redacted: true,
    }
}
