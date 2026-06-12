use super::*;
use temper_process_protocol::{
    WorkflowExternalToolManifest, WorkflowPromptManifest, WorkflowPromptSection,
    WorkflowRoleDecisionRequest, WorkflowRoleManifest, WorkflowRolePromptExtension,
    WorkflowToolManifest,
};

fn fixture_request() -> WorkflowRoleDecisionRequest {
    serde_json::from_str(include_str!(
        "../../../../temper/crates/temper-process-protocol/fixtures/workflow-role-decision-request.json"
    ))
    .expect("Temper workflow-role decision fixture parses")
}

fn request_with_compiled_external_tool(bound: bool) -> WorkflowRoleDecisionRequest {
    let manifest = WorkflowRoleManifest {
        id: "banana".to_string(),
        charter: None,
        prompt_extension: WorkflowRolePromptExtension {
            guidance: Some("Use open_pr only when coding_workspace is available.".to_string()),
            tool_guidance: None,
        },
        concurrency: None,
        queues: vec!["todo".to_string()],
        authority: Vec::new(),
        tools: vec![WorkflowToolManifest {
            name: "advance".to_string(),
            transition: "advance".to_string(),
            artifact: "task".to_string(),
            requires_gates: Vec::new(),
            effects: Vec::new(),
        }],
        external_tools: vec![WorkflowExternalToolManifest {
            id: "coding_workspace".to_string(),
            description: "Edit and commit repository code.".to_string(),
            required: false,
            constraints: vec!["Only touch the checked-out repository.".to_string()],
            guidance: Some("Produce a real product diff.".to_string()),
        }],
        prompt: WorkflowPromptManifest {
            role: "banana".to_string(),
            sections: vec![
                WorkflowPromptSection {
                    heading: "Workflow".to_string(),
                    lines: vec!["Workflow: generic-agent-test".to_string()],
                },
                WorkflowPromptSection {
                    heading: "Role".to_string(),
                    lines: vec![
                        "Role: banana".to_string(),
                        "Use open_pr only when coding_workspace is available.".to_string(),
                    ],
                },
                WorkflowPromptSection {
                    heading: EXTERNAL_TOOL_SECTION.to_string(),
                    lines: vec!["coding_workspace is declared by the workflow.".to_string()],
                },
            ],
        },
    };
    let available = bound
        .then(|| BoundExternalTool {
            id: "coding_workspace".to_string(),
            description: "Edit and commit repository code.".to_string(),
            required: false,
            constraints: vec!["Only touch the checked-out repository.".to_string()],
            guidance: Some("Produce a real product diff.".to_string()),
            provider: "workspace-local".to_string(),
        })
        .into_iter()
        .collect();
    WorkflowRoleDecisionRequest::new(
        "generic-agent-test",
        manifest,
        serde_json::json!({"artifact": {"number": 1}, "queue": "todo"}),
        available,
    )
}

#[test]
fn reads_temper_process_fixture_and_builds_generic_context() {
    let request = fixture_request();
    let prompt = workflow_role_system_prompt(&request);
    let context: serde_json::Value =
        serde_json::from_str(&workflow_role_user_context(&request).expect("context serializes"))
            .expect("context is JSON");

    assert_eq!(
        request.protocol_version,
        WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION
    );
    assert!(prompt.contains("Workflow: generic-agent-test"));
    assert!(prompt.contains("Role: banana"));
    assert_eq!(
        context["allowed_actions"],
        serde_json::json!(["no_action", "advance"])
    );
    assert_eq!(context["work_item"]["artifact"]["number"], 1);
    assert_eq!(context["authorized_actions"][0]["action"], "advance");
    assert_eq!(
        context["available_external_tools"][0]["provider"],
        "workspace-local"
    );
}

#[test]
fn runtime_prompt_lists_only_bound_external_tools() {
    let unbound = workflow_role_system_prompt(&request_with_compiled_external_tool(false));
    assert!(unbound.contains("no external tools are bound"));
    assert!(!unbound.contains("coding_workspace via"));

    let bound = workflow_role_system_prompt(&request_with_compiled_external_tool(true));
    assert!(bound.contains("coding_workspace via workspace-local"));
    // The engineer must learn that selecting the PR-opening action runs the
    // bound workspace; it must not decline because it cannot run the tool itself.
    assert!(bound.contains("selecting the PR-opening workflow action runs it"));
    assert!(bound.contains("do not return no_action expecting to run the workspace first"));
    assert!(bound.contains("selecting the workflow action a bound tool backs makes the engine run that tool automatically"));
    assert!(bound.contains("Produce a real product diff."));
}

#[test]
fn authorized_and_no_action_model_decisions_echo_request_version() {
    let request = fixture_request();
    let action = reply_for_model_decision(
        &request,
        WorkflowRoleModelDecision::action("advance", "ready"),
    );
    assert_eq!(action.protocol_version, request.protocol_version);
    assert_eq!(action.action, "advance");
    assert_eq!(action.reason, "ready");

    let none = reply_for_model_decision(&request, WorkflowRoleModelDecision::no_action("not safe"));
    assert_eq!(none.action, WORKFLOW_ROLE_DECISION_NO_ACTION);
    assert_eq!(none.reason, "not safe");
}

#[test]
fn unauthorized_model_action_is_returned_as_no_action() {
    let request = fixture_request();
    let reply = reply_for_model_decision(
        &request,
        WorkflowRoleModelDecision::action("delete_everything", "bad"),
    );

    assert_eq!(reply.protocol_version, request.protocol_version);
    assert_eq!(reply.action, WORKFLOW_ROLE_DECISION_NO_ACTION);
    assert!(reply.reason.contains("delete_everything"));
}

#[test]
fn unauthorized_model_action_reason_is_redacted() {
    let request = fixture_request();
    let reply = reply_for_model_decision(
        &request,
        WorkflowRoleModelDecision::action("sk-secret-do-not-log", "bad"),
    );

    assert_eq!(reply.action, WORKFLOW_ROLE_DECISION_NO_ACTION);
    assert!(!reply.reason.contains("sk-secret-do-not-log"));
    assert!(reply.reason.contains("<redacted>"));
}

#[test]
fn unauthorized_model_action_records_downgrade_metadata() {
    let request = fixture_request();
    let validated = validated_reply_for_model_decision(
        &request,
        WorkflowRoleModelDecision::action("delete_everything", "bad"),
    );

    assert_eq!(validated.reply.action, WORKFLOW_ROLE_DECISION_NO_ACTION);
    assert_eq!(
        validated.log_metadata.model_action.as_deref(),
        Some("delete_everything")
    );
    assert_eq!(
        validated.log_metadata.unauthorized_model_action.as_deref(),
        Some("delete_everything")
    );
    assert_eq!(
        validated.log_metadata.outcome,
        "unauthorized_action_downgraded"
    );
}

#[test]
fn rejects_unknown_protocol_version_before_model_call() {
    let mut request = fixture_request();
    request.protocol_version = 999;

    let error = validate_request_version(&request).expect_err("version fails");
    assert!(matches!(
        error,
        WorkflowRoleDecisionError::UnsupportedProtocolVersion { actual: 999 }
    ));
}
