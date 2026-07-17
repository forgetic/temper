//! Classification of provider run errors and the `ModelUnavailable` message.

use crate::coding_agent::*;
use temper_agent_core::{AgentOutcome, AgentStop};
use tongs::model::{AssistantMessage, ContentBlock, StopReason, TextContent, Usage};

#[test]
fn classifies_model_unavailable_from_provider_phrasings() {
    for message in [
        "api error (404): Claude Fable 5 is not available. Please use Opus 4.8.",
        "model `gpt-5.5` does not exist or you do not have access to it",
        "error: model_not_found",
    ] {
        match classify_run_error("claude-fable-5", message.to_string()) {
            CodingAgentError::ModelUnavailable { model, detail } => {
                assert_eq!(model, "claude-fable-5");
                assert_eq!(detail, message);
            }
            other => panic!("expected ModelUnavailable for {message:?}, got {other:?}"),
        }
    }
}

#[test]
fn classifies_other_errors_as_abnormal_stop() {
    match classify_run_error("claude-opus-4-8", "http error: write zero".to_string()) {
        CodingAgentError::AgentStopped(reason) => assert_eq!(reason, "http error: write zero"),
        other => panic!("expected AgentStopped, got {other:?}"),
    }
}

#[test]
fn model_unavailable_message_points_at_overrides() {
    let rendered = CodingAgentError::ModelUnavailable {
        model: "claude-fable-5".to_string(),
        detail: "404 not available".to_string(),
    }
    .to_string();
    assert!(rendered.contains("claude-fable-5"));
    assert!(rendered.contains("--model"));
}

#[test]
fn budget_exhaustion_rejects_parseable_result_text_with_typed_limit() {
    let outcome = outcome_with_result_text(AgentStop::BudgetExhausted);
    let error = ensure_completed_outcome(&outcome, "test-model", 7, false)
        .expect_err("budget exhaustion must precede result parsing");

    assert!(matches!(
        &error,
        CodingAgentError::BudgetExhausted { max_iterations: 7 }
    ));
    assert!(error.to_string().contains("budget_exhausted"));
}

#[test]
fn aborted_result_text_preserves_requested_and_unrequested_authority() {
    let outcome = outcome_with_result_text(AgentStop::Aborted);

    for (requested, expected) in [
        (false, AgentAbortAuthority::Unrequested),
        (true, AgentAbortAuthority::WorkerRequested),
    ] {
        let error = ensure_completed_outcome(&outcome, "test-model", 7, requested)
            .expect_err("aborted output must not be parsed");
        assert!(matches!(
            &error,
            CodingAgentError::Aborted { authority } if *authority == expected
        ));
        let rendered = error.to_string();
        assert!(rendered.contains("aborted"));
        assert!(rendered.contains(&expected.to_string()));
    }
}

fn outcome_with_result_text(stop: AgentStop) -> AgentOutcome {
    AgentOutcome {
        stop,
        final_message: AssistantMessage {
            content: vec![ContentBlock::Text(TextContent {
                text: r#"{"verdict":"needs_architect","summary":"looks complete"}"#.to_string(),
                text_signature: None,
            })],
            api: "test".to_string(),
            provider: "test".to_string(),
            model: "test-model".to_string(),
            usage: Usage::default(),
            stop_reason: StopReason::Aborted,
            error_message: None,
            timestamp: 0,
        },
        messages: Vec::new(),
    }
}
