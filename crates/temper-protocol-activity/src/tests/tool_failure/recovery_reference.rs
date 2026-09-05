use super::*;

#[test]
fn legacy_tool_start_without_disposition_remains_readable() {
    let legacy = serde_json::json!({
        "call_id": "legacy-bash",
        "name": "bash"
    });
    let parsed: ToolStartedV1 = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(parsed.arguments, None);
    assert_eq!(parsed.shell_discovery_disposition, None);
    assert_eq!(parsed.recovery_reference_disposition, None);
    assert_eq!(serde_json::to_value(parsed).unwrap(), legacy);
}

#[test]
fn recovery_reference_dispositions_are_closed_and_wrapper_only() {
    const PRIVATE_REFERENCE: &str = "temper-recovery-selector:00000000-0000-4000-8000-000000000099";
    let mut event = usage_event(1);
    event.event = AgentActivityEventV1::ToolStarted(ToolStartedV1 {
        call_id: "trace-reference".into(),
        name: "codebase_memory_trace_path".into(),
        arguments: None,
        shell_discovery_disposition: None,
        recovery_reference_disposition: Some(GraphRecoveryReferenceDispositionV1::Recognized),
    });
    event
        .validate()
        .expect("recognized trace reference validates");
    assert!(
        !serde_json::to_string(&event)
            .unwrap()
            .contains(PRIVATE_REFERENCE)
    );

    let AgentActivityEventV1::ToolStarted(started) = &mut event.event else {
        unreachable!();
    };
    started.name = "codebase_memory_get_code_snippet".into();
    event
        .validate()
        .expect("recognized source reference validates");
    let AgentActivityEventV1::ToolStarted(started) = &mut event.event else {
        unreachable!();
    };
    started.recovery_reference_disposition = Some(GraphRecoveryReferenceDispositionV1::Rejected);
    event
        .validate()
        .expect("rejected source reference validates");
    let AgentActivityEventV1::ToolStarted(started) = &mut event.event else {
        unreachable!();
    };
    started.recovery_reference_disposition = Some(GraphRecoveryReferenceDispositionV1::Expanded);
    assert_code(event.validate(), ActivityValidationCode::InvalidEvent);

    event.event = AgentActivityEventV1::ToolFinished(ToolFinishedV1 {
        call_id: "trace-reference".into(),
        name: "codebase_memory_trace_path".into(),
        status: ToolStatusV1::Succeeded,
        duration_ms: 1,
        result: None,
        failure: None,
        codebase_memory_timing: None,
        graph_correlation: None,
        decision_anchor_lineage: None,
        recovery_reference_disposition: Some(GraphRecoveryReferenceDispositionV1::Expanded),
    });
    event
        .validate()
        .expect("expanded trace reference validates");
    let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
        unreachable!();
    };
    finished.recovery_reference_disposition = Some(GraphRecoveryReferenceDispositionV1::Recognized);
    assert_code(event.validate(), ActivityValidationCode::InvalidEvent);
}
