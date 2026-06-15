//! Pure reply/error classification tests for the decision adapter.

use super::*;

use std::time::Duration;

use crate::WorkflowRoleDecisionProtocolError;

#[test]
fn decision_reply_classification_distinguishes_adapter_branches() {
    let request: WorkflowRoleDecisionRequest = serde_json::from_str(include_str!(
        "../../fixtures/workflow-role-decision-request.json"
    ))
    .expect("request fixture parses");

    let authorized = classify_decision_reply(
        &request,
        &WorkflowRoleDecisionReply::action("advance", "authorized"),
    );
    assert_eq!(authorized.validation_outcome, "valid");
    assert_eq!(authorized.action_kind, "authorized_action");
    assert_eq!(authorized.disposition, DecisionDisposition::ExecuteAction);

    let no_action = classify_decision_reply(
        &request,
        &WorkflowRoleDecisionReply::no_action("nothing useful to do"),
    );
    assert_eq!(no_action.validation_outcome, "valid");
    assert_eq!(no_action.action_kind, "no_action");
    assert_eq!(no_action.disposition, DecisionDisposition::NoAction);

    let unauthorized = classify_decision_reply(
        &request,
        &WorkflowRoleDecisionReply::action("delete_everything", "bad idea"),
    );
    assert_eq!(
        unauthorized.validation_outcome,
        "unauthorized_downgraded_to_no_action"
    );
    assert_eq!(unauthorized.action_kind, "no_action");
    assert_eq!(unauthorized.disposition, DecisionDisposition::NoAction);

    let mismatch = classify_decision_reply(
        &request,
        &WorkflowRoleDecisionReply {
            protocol_version: 999,
            action: "advance".to_string(),
            reason: "old responder".to_string(),
        },
    );
    assert_eq!(mismatch.validation_outcome, "protocol_mismatch");
    assert_eq!(mismatch.action_kind, "invalid_reply");
    assert_eq!(mismatch.disposition, DecisionDisposition::Error);
    assert!(
        mismatch
            .error
            .expect("mismatch keeps protocol error")
            .contains("version mismatch")
    );
}

#[test]
fn process_error_classification_distinguishes_failure_branches() {
    let malformed_json = serde_json::from_str::<WorkflowRoleDecisionReply>("not-json")
        .expect_err("bad reply is malformed");
    assert_eq!(
        classify_process_error(&WorkflowRoleDecisionProcessError::MalformedJson {
            source: malformed_json,
        }),
        ("malformed_json", "invalid_reply")
    );
    assert_eq!(
        classify_process_error(&WorkflowRoleDecisionProcessError::Timeout {
            timeout: Duration::from_millis(10),
        }),
        ("timeout", "process_unavailable")
    );
    assert_eq!(
        classify_process_error(&WorkflowRoleDecisionProcessError::Protocol(
            WorkflowRoleDecisionProtocolError::VersionMismatch {
                expected: 1,
                actual: 999,
            },
        )),
        ("protocol_mismatch", "invalid_reply")
    );
    assert_eq!(
        classify_process_error(&WorkflowRoleDecisionProcessError::Exit {
            status: "exit status: 7".to_string(),
            stderr: "stderr preview".to_string(),
        }),
        ("process_failure", "process_unavailable")
    );
}
