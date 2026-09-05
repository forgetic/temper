fn fail_correction_preview(machine: &mut AgentMachine, correction_references: &[String]) {
    let failed_preview = complete_llm(
        machine,
        assistant(vec![(
            "failed-correction-preview",
            "codebase_memory_get_code_snippet",
            serde_json::json!({"qualified_name": correction_references[0]}),
        )]),
    );
    assert!(failed_preview.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool { call, denial: None, rejection: None, .. }
            if call.id == "failed-correction-preview"
    )));
    let after_failed_preview = complete_tool(
        machine,
        "failed-correction-preview",
        failed_output(),
        Some(ToolFailureDiagnostic::codebase_memory(
            ToolFailureCategory::Transport,
        )),
    );
    assert!(!after_failed_preview
        .iter()
        .any(|request| matches!(request, AgentRequest::Finished { .. })));
    assert_eq!(
        handoff_references(active_handoff(&after_failed_preview)),
        correction_references,
        "a failed preview keeps the complete bounded handoff retryable",
    );
}
