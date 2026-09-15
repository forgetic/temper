// Independent companion authority must never revive retired graph selectors.
fn assert_retained_companion_read_preserves_retired_selector(
    machine: &mut AgentMachine,
    retired_reference: &str,
) {
    // The rejected implementation gains no mutation authority from its preview.
    // Only this later independent read can admit it as an exact companion.
    let unchosen_read = complete_llm(
        machine,
        assistant(vec![(
            "unchosen-target-read",
            "read",
            serde_json::json!({"path":"demo/src/model.rs"}),
        )]),
    );
    let _ = complete_tool(
        machine,
        "unchosen-target-read",
        successful_output(),
        None,
    );
    let retired_selector = complete_llm(
        machine,
        assistant(vec![(
            "correction-after-companion-read",
            "codebase_memory_get_code_snippet",
            serde_json::json!({
                "qualified_name": retired_reference,
                "decision_evidence_kind": "implementation",
            }),
        )]),
    );
    assert!(retired_selector.iter().any(|request| matches!(
        request,
        AgentRequest::Emit(AgentEvent::ToolStart {
            recovery_reference_disposition: Some(
                GraphRecoveryReferenceDispositionV1::Rejected
            ),
            ..
        })
    )));
    let retired_failure =
        local_failure(&retired_selector, "correction-after-companion-read");
    let _ = complete_tool(
        machine,
        "correction-after-companion-read",
        failed_output(),
        Some(retired_failure),
    );

    let companion_mutation = complete_llm(
        machine,
        assistant(vec![(
            "companion-target-mutation",
            "write",
            serde_json::json!({"path":"demo/src/model.rs","content":"changed"}),
        )]),
    );
    assert!(companion_mutation.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool { call, denial: None, rejection: None, .. }
            if call.id == "companion-target-mutation"
    )));
    let _ = complete_tool(
        machine,
        "companion-target-mutation",
        successful_output(),
        None,
    );

    assert!(unchosen_read.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool { call, denial: None, rejection: None, .. }
            if call.id == "unchosen-target-read"
    )));
}
