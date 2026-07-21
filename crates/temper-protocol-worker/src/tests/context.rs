// SPDX-License-Identifier: MPL-2.0

use crate::{
    ContextOutcome, ContextResponse, FetchContext, ForgeContextErrorCode, ForgeContextOperation,
    ForgeGetItemOperation, WorkerProtocolMessage,
};

#[test]
fn fetch_context_and_stable_error_round_trip() {
    let request = FetchContext::new(
        "worker-a",
        "job-1",
        "attempt-1",
        ForgeContextOperation::ForgeGetItem(ForgeGetItemOperation {
            repo: "ai/temper".to_string(),
            number: 283,
            artifact_type: None,
            include_comments: false,
        }),
    );
    let message = WorkerProtocolMessage::FetchContext(request.clone());
    let json = serde_json::to_value(&message).expect("serializes");
    assert_eq!(json["type"], "fetch-context");
    assert_eq!(json["attempt_id"], "attempt-1");
    assert_eq!(json["operation"]["operation"], "forge_get_item");
    assert_eq!(
        serde_json::from_value::<WorkerProtocolMessage>(json).expect("parses"),
        message
    );

    let response = WorkerProtocolMessage::ContextResponse(ContextResponse::error(
        &request,
        ForgeContextErrorCode::NotAuthorized,
    ));
    let json = serde_json::to_value(&response).expect("serializes");
    assert_eq!(json["type"], "context-response");
    assert_eq!(json["status"], "error");
    assert_eq!(json["code"], "not_authorized");
    assert!(json.get("result").is_none());
    assert!(matches!(
        serde_json::from_value::<WorkerProtocolMessage>(json).expect("parses"),
        WorkerProtocolMessage::ContextResponse(ContextResponse {
            outcome: ContextOutcome::Error {
                code: ForgeContextErrorCode::NotAuthorized
            },
            ..
        })
    ));
}

#[test]
fn legacy_fetch_context_without_attempt_remains_readable() {
    let request: FetchContext = serde_json::from_str(
        r#"{"protocol_version":1,"worker_id":"legacy","job_id":"job-1","operation":{"operation":"forge_get_item","repo":"ai/temper","number":1,"include_comments":false}}"#,
    )
    .expect("legacy request");
    assert_eq!(request.attempt_id, None);
}
