//! Classification of provider run errors and the `ModelUnavailable` message.

use crate::coding_agent::*;

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
    assert!(rendered.contains("TEMPER_AGENTS_ANTHROPIC_MODEL"));
}
