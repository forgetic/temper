use super::*;

#[test]
fn failed_finish_retains_safe_diagnostics_and_retry_summary_in_every_capture_mode() {
    let upstream = ProviderFailureDiagnostic::new(
        FailureCategory::RateLimit,
        true,
        Some(429),
        Some("req_activity_530"),
        Some("rate_limit_exceeded"),
        "Please retry later.",
    );
    let diagnostic =
        ModelFailureDiagnostic::from_provider(&ModelIdentity::new("provider", "model"), &upstream);

    for mode in [
        CaptureModeV1::Metadata,
        CaptureModeV1::Transcript,
        CaptureModeV1::Diagnostic,
    ] {
        let recorder = Arc::new(Recorder::default());
        let factory = ScopeFactory::with_parts(
            AgentActivityCapturePolicyV1 {
                capture: mode,
                capture_thinking: mode == CaptureModeV1::Diagnostic,
                ..Default::default()
            },
            Arc::new(FakeClock::new(0..20)),
            vec![recorder.clone()],
        );
        let run = factory.main("main", ModelIdentity::new("provider", "model"));
        run.observability
            .events
            .emit(AgentEvent::ModelCallFinished {
                turn: 3,
                call_id: "model-call-530".to_string(),
                attempt: 3,
                status: ModelCallStatus::Failed,
                duration_ms: 700,
                time_to_first_token_ms: Some(120),
                stop_reason: Some(StopReason::Error),
                usage: Default::default(),
                failure: Some(diagnostic.clone()),
            });
        run.observability
            .events
            .emit(AgentEvent::ModelCallRetrying {
                turn: 3,
                call_id: "model-call-530".to_string(),
                next_attempt: 4,
                delay_ms: 750,
                reason: diagnostic.clone(),
            });

        let frames = recorder.0.lock().expect("frames");
        let finished = frames
            .iter()
            .find_map(|frame| match &frame.event {
                AgentActivityEventV1::ModelCallFinished(finished) => Some(finished),
                _ => None,
            })
            .expect("failed finish boundary");
        assert_eq!(finished.status, ModelCallStatusV1::Failed);
        assert_eq!(finished.stop_reason, Some(StopReasonV1::Error));
        let failure = finished.failure.as_ref().expect("safe diagnostic");
        assert_eq!(failure.provider, "provider");
        assert_eq!(failure.model, "model");
        assert_eq!(failure.category, ModelFailureCategoryV1::RateLimit);
        assert!(failure.retryable);
        assert_eq!(failure.http_status, Some(429));
        assert_eq!(
            failure.provider_request_id.as_deref(),
            Some("req_activity_530")
        );
        assert_eq!(
            failure.provider_error_code.as_deref(),
            Some("rate_limit_exceeded")
        );
        assert_eq!(
            failure.message,
            temper_protocol_activity::REDACTED_MODEL_FAILURE_MESSAGE
        );
        assert!(failure.detail_redacted);

        let retry = frames
            .iter()
            .find_map(|frame| match &frame.event {
                AgentActivityEventV1::ModelCallRetrying(retry) => Some(retry),
                _ => None,
            })
            .expect("retry boundary");
        assert_eq!(retry.call_id, "model-call-530");
        assert_eq!(retry.next_attempt, 4);
        assert_eq!(retry.delay_ms, 750);
        assert_eq!(
            retry.failure.code,
            temper_protocol_activity::FailureCodeV1::Provider
        );
        assert!(retry.failure.retryable);
        assert_eq!(retry.failure.message, MODEL_CALL_RETRY_FAILURE_MESSAGE);
        assert!(!retry.failure.message.contains("Please retry later."));
    }
}

#[test]
fn first_party_model_finishes_enforce_failure_consistency() {
    let recorder = Arc::new(Recorder::default());
    let factory = ScopeFactory::with_parts(
        AgentActivityCapturePolicyV1::default(),
        Arc::new(FakeClock::new(0..20)),
        vec![recorder.clone()],
    );
    let run = factory.main(
        "main",
        ModelIdentity::new("first-party-provider", "first-party-model"),
    );
    let diagnostic = ModelFailureDiagnostic::redacted_unknown("provider", "model", false);

    run.observability
        .events
        .emit(AgentEvent::ModelCallFinished {
            turn: 0,
            call_id: "failed-without-detail".into(),
            attempt: 0,
            status: ModelCallStatus::Failed,
            duration_ms: 10,
            time_to_first_token_ms: None,
            stop_reason: None,
            usage: Default::default(),
            failure: None,
        });
    run.observability
        .events
        .emit(AgentEvent::ModelCallFinished {
            turn: 1,
            call_id: "success-with-detail".into(),
            attempt: 0,
            status: ModelCallStatus::Succeeded,
            duration_ms: 10,
            time_to_first_token_ms: None,
            stop_reason: Some(StopReason::Stop),
            usage: Default::default(),
            failure: Some(diagnostic.clone()),
        });
    run.observability
        .events
        .emit(AgentEvent::ModelCallFinished {
            turn: 2,
            call_id: "contradictory-success".into(),
            attempt: 0,
            status: ModelCallStatus::Succeeded,
            duration_ms: 10,
            time_to_first_token_ms: None,
            stop_reason: Some(StopReason::Error),
            usage: Default::default(),
            failure: None,
        });

    let frames = recorder.0.lock().expect("frames");
    let finishes = frames
        .iter()
        .filter_map(|frame| match &frame.event {
            AgentActivityEventV1::ModelCallFinished(finished) => Some(finished),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(finishes.len(), 3);
    assert_eq!(finishes[0].status, ModelCallStatusV1::Failed);
    let fallback = finishes[0].failure.as_ref().expect("failed fallback");
    assert_eq!(fallback.provider, "first-party-provider");
    assert_eq!(fallback.model, "first-party-model");
    assert_eq!(fallback.category, ModelFailureCategoryV1::RedactedUnknown);
    assert_eq!(finishes[1].status, ModelCallStatusV1::Succeeded);
    assert_eq!(finishes[1].failure, None);
    assert_eq!(finishes[2].status, ModelCallStatusV1::Failed);
    assert_eq!(finishes[2].stop_reason, Some(StopReasonV1::Error));
    assert!(finishes[2].failure.is_some());
}
