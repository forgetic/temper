use crate::{
    ToolFailureCategoryV1, ToolFailureDiagnosticV1, ToolFailureReasonV1, ToolRetryDispositionV1,
};

#[test]
fn mutation_diagnostics_round_trip_closed_reasons_and_discard_untrusted_messages() {
    for (reason, text) in [
        (
            ToolFailureReasonV1::MalformedMutationTarget,
            "malformed_mutation_target",
        ),
        (
            ToolFailureReasonV1::ConflictingMutationTargets,
            "conflicting_mutation_targets",
        ),
    ] {
        let diagnostic = ToolFailureDiagnosticV1::with_reason(
            ToolFailureCategoryV1::SchemaArgumentMismatch,
            reason,
        );
        assert_eq!(diagnostic.reason, reason);
        assert_eq!(
            diagnostic.retry_disposition,
            ToolRetryDispositionV1::CorrectInvocation
        );
        assert!(!diagnostic.retryable);
        assert!(!diagnostic.fallback_to_conventional_discovery);
        assert!(diagnostic.message.contains("correct the invocation"));
        let mut wire = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(wire["reason"], text);
        wire["message"] = serde_json::json!("PRIVATE-PATCH CREDENTIAL /private/path");
        wire["retry_disposition"] = serde_json::json!("retryable");
        wire["retryable"] = serde_json::json!(true);
        let restored: ToolFailureDiagnosticV1 = serde_json::from_value(wire).unwrap();
        assert_eq!(restored, diagnostic);
        let rendered = serde_json::to_string(&restored).unwrap();
        for private in ["PRIVATE-PATCH", "CREDENTIAL", "/private/path"] {
            assert!(!rendered.contains(private));
        }
    }
}
