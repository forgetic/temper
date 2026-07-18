use super::*;
use temper_process_containment::{CleanupReport, ContainmentIdentity, ContainmentRootIdentity};

#[derive(Default)]
struct RecordingProjection(Mutex<Vec<(AgentLifecycleScopeV1, AgentLifecycleEventV1)>>);

impl LifecycleProjection for RecordingProjection {
    fn emit(&self, scope: AgentLifecycleScopeV1, event: AgentLifecycleEventV1) {
        self.0.lock().expect("events").push((scope, event));
    }
}

fn blocked_observation() -> CleanupObservation {
    CleanupObservation::new(
        ContainmentIdentity::new("bash-call-42")
            .unwrap()
            .with_owner_identifier("bash-call-42")
            .unwrap(),
        ContainmentScope::Tool,
        ContainmentBackendKind::LinuxSupervisor,
        ContainmentRootIdentity::new(
            ContainmentBackendKind::LinuxSupervisor,
            "root credential=secret-token-sentinel",
        ),
        CleanupSnapshot::Blocked {
            trigger: CleanupTrigger::Cancellation,
            phase: CleanupPhase::VerifyEmpty,
            message: "bash -c 'curl -H authorization: bearer credential'".to_string(),
            survivors: vec![ProcessIdentity::new(
                42,
                1,
                42,
                42,
                100,
                "/tmp/password=secret-token-sentinel/temper-agent",
            )],
            omitted_survivors: 7,
        },
    )
}

#[test]
fn nested_cleanup_is_throttled_content_free_and_lifecycle_visible() {
    let projection = Arc::new(RecordingProjection::default());
    let observer = LifecycleCleanupObserver::with_interval(
        projection.clone(),
        AgentLifecycleScopeV1 {
            id: "containment".to_string(),
            parent_id: None,
        },
        Duration::from_secs(60),
    );
    let blocked = blocked_observation();

    observer.observe_cleanup(&blocked);
    observer.observe_cleanup(&blocked); // suppressed inside the interval
    observer.observe_cleanup(&blocked); // promoted third failure
    observer.observe_fallback(&ContainmentFallbackObservation::new(
        ContainmentIdentity::new("mcp-server")
            .unwrap()
            .with_owner_identifier("mcp-server")
            .unwrap(),
        ContainmentScope::McpServer,
        ContainmentBackendKind::LinuxSupervisor,
        ContainmentRootIdentity::new(ContainmentBackendKind::LinuxSupervisor, "mcp-root"),
        "cgroup preparation failed: token=secret-token-sentinel",
    ));
    observer.observe_cleanup(&CleanupObservation::new(
        ContainmentIdentity::new("bash-call-42").unwrap(),
        ContainmentScope::Tool,
        ContainmentBackendKind::NoProcess,
        ContainmentRootIdentity::new(ContainmentBackendKind::NoProcess, "not-spawned"),
        CleanupSnapshot::Completed {
            report: CleanupReport::no_process(CleanupTrigger::NormalRootExit),
        },
    ));

    let events = projection.0.lock().expect("events");
    assert_eq!(events.len(), 4);
    let AgentLifecycleEventV1::Containment {
        observation: AgentContainmentEventV1::CleanupBlocked(first),
    } = &events[0].1
    else {
        panic!("expected first blocked cleanup")
    };
    assert_eq!(first.repeated_failures, 1);
    assert_eq!(first.owner.owner_kind, "tool");
    assert_eq!(first.owner.tool_command_id, "bash-call-42");
    let AgentLifecycleEventV1::Containment {
        observation: AgentContainmentEventV1::CleanupBlocked(third),
    } = &events[1].1
    else {
        panic!("expected promoted blocked cleanup")
    };
    assert_eq!(third.repeated_failures, 3);
    assert_eq!(third.omitted_survivors, 7);
    assert_eq!(third.survivors[0].executable, "[redacted]");
    assert!(matches!(
        events[2].1,
        AgentLifecycleEventV1::Containment {
            observation: AgentContainmentEventV1::FallbackActivated(_)
        }
    ));
    assert!(matches!(
        events[3].1,
        AgentLifecycleEventV1::Containment {
            observation: AgentContainmentEventV1::CleanupCompleted(_)
        }
    ));

    let wire = serde_json::to_string(&*events).expect("serialize lifecycle evidence");
    for forbidden in [
        "secret-token-sentinel",
        "authorization:",
        "bearer ",
        "credential=",
        "password=",
        "bash -c",
        "curl -H",
    ] {
        assert!(
            !wire.contains(forbidden),
            "lifecycle evidence leaked {forbidden}"
        );
    }
}
