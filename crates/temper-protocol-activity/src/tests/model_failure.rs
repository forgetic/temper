use super::*;

fn model_failure() -> ModelFailureV1 {
    ModelFailureV1 {
        provider: "openai-codex".into(),
        model: "gpt-5.6-sol".into(),
        category: ModelFailureCategoryV1::RateLimit,
        retryable: true,
        http_status: Some(429),
        provider_request_id: Some("req_01JTEST:attempt/2".into()),
        provider_error_code: Some("rate_limit_exceeded.v2".into()),
        message: "Provider rate limit exceeded.".into(),
        detail_redacted: false,
    }
}

#[test]
fn retained_model_finishes_without_diagnostics_remain_readable() {
    let events: Vec<AgentActivityEventV1> = round_trip(&fixture("model-call-finished-legacy.json"));
    assert_eq!(events.len(), 2);
    for (index, event) in events.into_iter().enumerate() {
        let AgentActivityEventV1::ModelCallFinished(finished) = &event else {
            panic!("legacy fixture must contain model.call.finished events");
        };
        assert_eq!(finished.failure, None);
        if index == 1 {
            assert_eq!(finished.status, ModelCallStatusV1::Succeeded);
            assert_eq!(finished.stop_reason, Some(StopReasonV1::Error));
        }
        let mut canonical = usage_event(index as u64 + 1);
        canonical.event = event;
        canonical
            .validate()
            .expect("retained model finish validates");
    }
}

#[test]
fn model_failure_vocabulary_and_safe_fields_round_trip() {
    let value = model_failure();
    value.validate().expect("safe diagnostic validates");
    assert_eq!(
        serde_json::to_value([
            ModelFailureCategoryV1::Timeout,
            ModelFailureCategoryV1::Transport,
            ModelFailureCategoryV1::RateLimit,
            ModelFailureCategoryV1::Authentication,
            ModelFailureCategoryV1::Context,
            ModelFailureCategoryV1::Response,
            ModelFailureCategoryV1::Provider,
            ModelFailureCategoryV1::RedactedUnknown,
        ])
        .unwrap(),
        json!([
            "timeout",
            "transport",
            "rate_limit",
            "authentication",
            "context",
            "response",
            "provider",
            "redacted_unknown"
        ])
    );

    let events: Vec<AgentActivityEventV1> = round_trip(&fixture("event-families.json"));
    let AgentActivityEventV1::ModelCallFinished(finished) = &events[9] else {
        panic!("model finish fixture");
    };
    let failure = finished.failure.as_ref().expect("canonical failure");
    assert_eq!(failure.category, ModelFailureCategoryV1::RateLimit);
    assert_eq!(failure.http_status, Some(429));
    assert_eq!(failure.provider_request_id.as_deref(), Some("req_01JTEST"));
}

#[test]
fn model_failure_validation_enforces_bounds_and_strict_identifiers() {
    let mut failure = model_failure();
    failure.provider = "p".repeat(MAX_MODEL_FAILURE_PROVIDER_BYTES);
    failure.model = "m".repeat(MAX_MODEL_FAILURE_MODEL_BYTES);
    failure.provider_request_id = Some("r".repeat(MAX_MODEL_FAILURE_REQUEST_ID_BYTES));
    failure.provider_error_code = Some("c".repeat(MAX_MODEL_FAILURE_PROVIDER_CODE_BYTES));
    failure.message = "x".repeat(MAX_MODEL_FAILURE_MESSAGE_BYTES);
    failure.validate().expect("values at every bound validate");

    for invalid in ["request id", "réquest", "request#1"] {
        let mut failure = model_failure();
        failure.provider_request_id = Some(invalid.into());
        assert_code(failure.validate(), ActivityValidationCode::InvalidEvent);
    }
    for invalid in ["provider code", "provider/code", "cødé"] {
        let mut failure = model_failure();
        failure.provider_error_code = Some(invalid.into());
        assert_code(failure.validate(), ActivityValidationCode::InvalidEvent);
    }

    let mut failure = model_failure();
    failure.provider.push('!');
    assert_code(failure.validate(), ActivityValidationCode::InvalidEvent);
    let mut failure = model_failure();
    failure.message = " \n ".into();
    assert_code(failure.validate(), ActivityValidationCode::InvalidEvent);
    let mut failure = model_failure();
    failure.message = "x".repeat(MAX_MODEL_FAILURE_MESSAGE_BYTES + 1);
    assert_code(
        failure.validate(),
        ActivityValidationCode::OversizedInlineValue,
    );
    let mut failure = model_failure();
    failure.http_status = Some(99);
    assert_code(failure.validate(), ActivityValidationCode::InvalidEvent);
}

#[test]
fn model_failure_normalization_fails_closed_without_losing_safe_facts() {
    let secret = "Authorization: Bearer MODEL-FAILURE-SECRET";
    let mut failure = model_failure();
    failure.message = secret.into();
    failure.normalize();

    failure.validate().expect("fallback validates");
    assert_eq!(failure.provider, "openai-codex");
    assert_eq!(failure.model, "gpt-5.6-sol");
    assert_eq!(failure.category, ModelFailureCategoryV1::RedactedUnknown);
    assert!(failure.retryable);
    assert_eq!(failure.http_status, Some(429));
    assert_eq!(
        failure.provider_request_id.as_deref(),
        Some("req_01JTEST:attempt/2")
    );
    assert_eq!(failure.provider_error_code, None);
    assert_eq!(failure.message, REDACTED_MODEL_FAILURE_MESSAGE);
    assert!(failure.detail_redacted);
    assert!(!serde_json::to_string(&failure).unwrap().contains(secret));

    failure.provider = "unsafe provider".into();
    failure.normalize();
    assert_eq!(failure.provider, UNKNOWN_MODEL_FAILURE_IDENTITY);
}

#[test]
fn model_finish_diagnostics_are_restricted_to_failed_status() {
    let mut canonical = usage_event(1);
    canonical.event = AgentActivityEventV1::ModelCallFinished(ModelCallFinishedV1 {
        call_id: "model-1".into(),
        attempt: 0,
        status: ModelCallStatusV1::Succeeded,
        duration_ms: 10,
        time_to_first_token_ms: None,
        stop_reason: Some(StopReasonV1::EndTurn),
        failure: Some(model_failure()),
    });
    assert_code(canonical.validate(), ActivityValidationCode::InvalidEvent);

    let AgentActivityEventV1::ModelCallFinished(finished) = &mut canonical.event else {
        unreachable!();
    };
    finished.status = ModelCallStatusV1::Failed;
    canonical
        .validate()
        .expect("failed call accepts diagnostic");
    let AgentActivityEventV1::ModelCallFinished(finished) = &mut canonical.event else {
        unreachable!();
    };
    finished.failure = None;
    canonical
        .validate()
        .expect("retained failed call without diagnostic remains readable");
}
