use crate::{
    MAX_SHUTDOWN_IDENTIFIER_BYTES, MAX_SHUTDOWN_SURVIVOR_PIDS, ShutdownBlocker,
    ShutdownBlockerKind, ShutdownEscalationStage,
};

#[test]
fn shutdown_blocker_vocabulary_and_bounds_are_stable() {
    let blocker = ShutdownBlocker::new(
        ShutdownBlockerKind::Containment,
        ShutdownEscalationStage::EmergencyKill,
        "mcp_server",
        "credential=secret-token-sentinel",
    )
    .with_identity(Some("worker"), Some("job"), Some("attempt"))
    .with_containment(
        Some(&"r".repeat(2_000)),
        Some(42),
        Some("verify_empty"),
        1..=u32::try_from(MAX_SHUTDOWN_SURVIVOR_PIDS + 3).unwrap(),
        2,
    )
    .with_trace(
        Some(&"t".repeat(MAX_SHUTDOWN_IDENTIFIER_BYTES + 1)),
        Some(9),
    )
    .with_timing(10, 20, 30);

    assert_eq!(blocker.kind.as_str(), "containment");
    assert_eq!(blocker.escalation_stage.as_str(), "emergency_kill");
    assert_eq!(blocker.owner_name, "[redacted]");
    assert_eq!(blocker.root_pid, Some(42));
    assert_eq!(blocker.survivor_pids.len(), MAX_SHUTDOWN_SURVIVOR_PIDS);
    assert_eq!(blocker.omitted_survivor_pids, 5);
    assert_eq!(
        blocker.trace_run_id.as_deref().unwrap().len(),
        MAX_SHUTDOWN_IDENTIFIER_BYTES
    );

    let wire = serde_json::to_string(&blocker).unwrap();
    for value in [
        "containment",
        "terminal_trace_ack",
        "result_delivery",
        "component_task",
        "registry_state",
    ] {
        let kind = match value {
            "containment" => ShutdownBlockerKind::Containment,
            "terminal_trace_ack" => ShutdownBlockerKind::TerminalTraceAck,
            "result_delivery" => ShutdownBlockerKind::ResultDelivery,
            "component_task" => ShutdownBlockerKind::ComponentTask,
            "registry_state" => ShutdownBlockerKind::RegistryState,
            _ => unreachable!(),
        };
        assert_eq!(kind.as_str(), value);
    }
    assert!(!wire.contains("secret-token-sentinel"));
    assert!(!wire.contains("credential="));
}
