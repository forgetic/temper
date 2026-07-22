use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use temper_process_containment::{
    CleanupPhase, CleanupReport, CleanupSnapshot, ContainmentBackendKind, ContainmentIdentity,
    ContainmentRootIdentity, ContainmentScope, ProcessIdentity,
};
use temper_protocol_agent::{
    AgentContainmentBackendV1, AgentContainmentCleanupBlockedV1, AgentContainmentEventV1,
    AgentContainmentOwnerV1, AgentContainmentPhaseV1, AgentContainmentProcessV1,
    AgentContainmentTriggerV1,
};
use tracing_subscriber::fmt::MakeWriter;

use super::*;

#[derive(Default)]
struct RecordingObserver(Mutex<Vec<ContainmentEvent>>);

impl ContainmentEventObserver for RecordingObserver {
    fn observe(&self, event: &ContainmentEvent) {
        self.0.lock().expect("events").push(event.clone());
    }
}

#[test]
fn startup_capability_uses_the_injected_observer() {
    let observer = RecordingObserver::default();
    observe_startup_containment_capability("worker-startup", &observer);
    let events = observer.0.lock().expect("events");
    let event = events
        .iter()
        .find_map(|event| match event {
            ContainmentEvent::StartupCapability(event) => Some(event),
            _ => None,
        })
        .expect("expected startup capability");
    assert_eq!(event.worker_id, "worker-startup");
    assert!(!event.selected_backend.is_empty());
    assert!(!event.cgroup_v2_mount.is_empty());
    assert!(!event.fallback_reason.is_empty());
}

#[test]
fn cleanup_events_have_expected_severity_bounded_evidence_and_redaction() {
    let context = ContainmentEventContext::new("worker-1", "job-1", "attempt-1");
    let identity = ContainmentIdentity::new("owner-0001")
        .unwrap()
        .with_owner_identifier("cargo-test")
        .unwrap();
    let root = ContainmentRootIdentity::new(
        ContainmentBackendKind::LinuxSupervisor,
        format!("supervisor:{}", "r".repeat(2_000)),
    );
    let survivors = (0..(MAX_EVENT_SURVIVORS + 7))
        .map(|offset| {
            let executable = if offset == 0 {
                PathBuf::from("/tmp/secret-token-sentinel")
            } else {
                PathBuf::from(format!("/very/long/{}/temper-agent", "x".repeat(500)))
            };
            ProcessIdentity::new(
                10_000 + offset as u32,
                9_000 + offset as u32,
                8_000 + offset as u32,
                7_000 + offset as u32,
                1_000_000 + offset as u64,
                executable,
            )
        })
        .collect::<Vec<_>>();
    let blocked = CleanupObservation::new(
        identity.clone(),
        ContainmentScope::Tool,
        ContainmentBackendKind::LinuxSupervisor,
        root.clone(),
        10_000,
        CleanupSnapshot::Blocked {
            trigger: CleanupTrigger::Cancellation,
            phase: CleanupPhase::VerifyEmpty,
            message: "credential=secret-token-sentinel output body".to_string(),
            survivors,
            omitted_survivors: 2,
        },
    );
    let shutdown_blocked = CleanupObservation::new(
        identity.clone(),
        ContainmentScope::Tool,
        ContainmentBackendKind::LinuxSupervisor,
        root.clone(),
        10_000,
        CleanupSnapshot::Blocked {
            trigger: CleanupTrigger::Shutdown,
            phase: CleanupPhase::Discover,
            message: "authorization: bearer secret-token-sentinel".to_string(),
            survivors: Vec::new(),
            omitted_survivors: 0,
        },
    );
    let completed = CleanupObservation::new(
        identity.clone(),
        ContainmentScope::Tool,
        ContainmentBackendKind::NoProcess,
        ContainmentRootIdentity::new(ContainmentBackendKind::NoProcess, "not-spawned"),
        0,
        CleanupSnapshot::Completed {
            report: CleanupReport::no_process(CleanupTrigger::NormalRootExit),
        },
    );
    let fallback = ContainmentFallbackObservation::new(
        identity,
        ContainmentScope::Tool,
        ContainmentBackendKind::LinuxSupervisor,
        root,
        "credential=secret-token-sentinel",
    );
    let startup = ContainmentCapabilityDiagnostic::new(
        Some("/sys/fs/cgroup".to_string()),
        false,
        false,
        false,
        true,
        ContainmentBackendKind::LinuxSupervisor,
        Some("token=secret-token-sentinel".to_string()),
    );

    let events = [
        ContainmentEvent::from_cleanup(&context, &blocked, 1, None, 0, 0).unwrap(),
        ContainmentEvent::from_cleanup(&context, &shutdown_blocked, 1, None, 0, 0).unwrap(),
        ContainmentEvent::from_cleanup(&context, &completed, 0, None, 0, 0).unwrap(),
        ContainmentEvent::from_fallback(&context, &fallback),
        ContainmentEvent::startup("worker-1", &startup),
    ];
    let captured = capture_events(|| {
        for event in &events {
            event.emit();
        }
    });
    assert_eq!(captured.len(), events.len());
    assert_eq!(captured[0]["level"], "WARN");
    assert_eq!(captured[1]["level"], "ERROR");
    assert_eq!(captured[2]["level"], "DEBUG");
    assert_eq!(captured[3]["level"], "WARN");
    assert_eq!(captured[4]["level"], "WARN");

    let fields = &captured[0]["fields"];
    assert_eq!(fields["worker_id"], "worker-1");
    assert_eq!(fields["job_id"], "job-1");
    assert_eq!(fields["attempt_id"], "attempt-1");
    assert_eq!(fields["owner_kind"], "tool");
    assert_eq!(fields["tool_command_id"], "cargo-test");
    assert_eq!(fields["backend"], "linux_supervisor");
    assert_eq!(fields["root_pid"], 10000);
    assert_eq!(captured[1]["fields"]["phase"], "discover");
    assert_eq!(captured[1]["fields"]["root_pid"], 10000);
    assert_eq!(fields["direct_child_reap"], "pending");
    assert_eq!(fields["recursive_empty"], "not_proven");
    assert!(fields["root"].as_str().unwrap().len() <= MAX_EVENT_ROOT_BYTES);

    let serialized_survivors: Vec<Value> =
        serde_json::from_str(fields["survivors"].as_str().unwrap()).unwrap();
    assert_eq!(serialized_survivors.len(), MAX_EVENT_SURVIVORS);
    for survivor in serialized_survivors {
        for required in [
            "pid",
            "ppid",
            "pgid",
            "session_id",
            "start_time",
            "executable",
        ] {
            assert!(survivor.get(required).is_some(), "missing {required}");
        }
        assert!(survivor["executable"].as_str().unwrap().len() <= MAX_EVENT_EXECUTABLE_BYTES);
    }

    let encoded = serde_json::to_string(&captured).unwrap();
    for forbidden in [
        "secret-token-sentinel",
        "credential=",
        "authorization:",
        "prompt_content",
        "command_arguments",
        "output body",
    ] {
        assert!(!encoded.contains(forbidden), "event leaked {forbidden}");
    }
}

#[test]
fn repeated_blocked_cleanup_is_throttled_by_root() {
    let observer = Arc::new(RecordingObserver::default());
    let throttle = ContainmentEventThrottle::new(observer.clone(), Duration::from_secs(60));
    let context = ContainmentEventContext::new("worker", "job", "attempt");
    let observation = CleanupObservation::new(
        ContainmentIdentity::new("tool").unwrap(),
        ContainmentScope::Tool,
        ContainmentBackendKind::LinuxSupervisor,
        ContainmentRootIdentity::new(ContainmentBackendKind::LinuxSupervisor, "root-1"),
        42,
        CleanupSnapshot::Blocked {
            trigger: CleanupTrigger::Cancellation,
            phase: CleanupPhase::Discover,
            message: "inspection failed".to_string(),
            survivors: Vec::new(),
            omitted_survivors: 0,
        },
    );
    let signal_observation = CleanupObservation::new(
        ContainmentIdentity::new("tool").unwrap(),
        ContainmentScope::Tool,
        ContainmentBackendKind::LinuxSupervisor,
        ContainmentRootIdentity::new(ContainmentBackendKind::LinuxSupervisor, "root-1"),
        42,
        CleanupSnapshot::SignalAttempted {
            trigger: CleanupTrigger::Cancellation,
            signal: temper_process_containment::ContainmentSignal::Term,
            attempts: vec![temper_process_containment::SignalAttempt::succeeded(
                ProcessIdentity::new(42, 1, 42, 42, 100, "/bin/tool"),
                temper_process_containment::ContainmentSignal::Term,
            )],
            omitted: 0,
        },
    );
    throttle.cleanup(&context, &signal_observation);
    throttle.cleanup(&context, &observation);
    throttle.cleanup(&context, &observation);
    assert_eq!(observer.0.lock().expect("events").len(), 1);
    // A repeated failure is promoted and emitted even inside the interval.
    throttle.cleanup(&context, &observation);
    let events = observer.0.lock().expect("events");
    assert_eq!(events.len(), 2);
    let ContainmentEvent::CleanupBlocked(event) = &events[1] else {
        panic!("expected blocked event")
    };
    assert_eq!(event.repeated_failures, 3);
    assert!(event.term_outcomes.contains("succeeded"));
    drop(events);
    throttle.cleanup(&context, &observation);
    assert_eq!(observer.0.lock().expect("events").len(), 2);
}

#[test]
fn blocker_age_uses_first_seen_across_throttled_emissions() {
    let observer = Arc::new(RecordingObserver::default());
    let now = Arc::new(Mutex::new(Duration::from_secs(5)));
    let clock_now = Arc::clone(&now);
    let throttle = ContainmentEventThrottle::with_clock(
        observer.clone(),
        Duration::from_secs(60),
        Arc::new(move || *clock_now.lock().expect("clock")),
    );
    let context = ContainmentEventContext::new("worker-age", "job-age", "attempt-age");
    let observation = CleanupObservation::new(
        ContainmentIdentity::new("age-owner").unwrap(),
        ContainmentScope::Tool,
        ContainmentBackendKind::LinuxSupervisor,
        ContainmentRootIdentity::new(ContainmentBackendKind::LinuxSupervisor, "age-root"),
        77,
        CleanupSnapshot::Blocked {
            trigger: CleanupTrigger::Shutdown,
            phase: CleanupPhase::Discover,
            message: "inspection failed".to_string(),
            survivors: Vec::new(),
            omitted_survivors: 0,
        },
    );

    throttle.cleanup(&context, &observation);
    *now.lock().expect("clock") = Duration::from_secs(10);
    throttle.cleanup(&context, &observation); // throttled
    *now.lock().expect("clock") = Duration::from_secs(20);
    throttle.cleanup(&context, &observation); // promoted third failure
    *now.lock().expect("clock") = Duration::from_secs(80);
    throttle.cleanup(&context, &observation); // interval elapsed

    let events = observer.0.lock().expect("events");
    let timings = events
        .iter()
        .map(|event| match event {
            ContainmentEvent::CleanupBlocked(event) => (event.first_seen_millis, event.age_millis),
            _ => panic!("expected blocked event"),
        })
        .collect::<Vec<_>>();
    assert_eq!(timings, vec![(5_000, 0), (5_000, 15_000), (5_000, 75_000)]);
}

#[test]
fn lifecycle_nested_cleanup_is_attempt_stamped_and_redacted() {
    let observer = Arc::new(RecordingObserver::default());
    let throttle = ContainmentEventThrottle::new(observer.clone(), Duration::from_secs(60));
    let context = ContainmentEventContext::new("worker-nested", "job-nested", "attempt-nested");
    let observation = AgentContainmentEventV1::CleanupBlocked(AgentContainmentCleanupBlockedV1 {
        owner: AgentContainmentOwnerV1 {
            owner_kind: "mcp_server".to_string(),
            tool_command_id: "credential=secret-token-sentinel".to_string(),
            backend: AgentContainmentBackendV1::LinuxSupervisor,
            root: "supervisor:nested".to_string(),
            root_pid: Some(41),
        },
        trigger: AgentContainmentTriggerV1::Cancellation,
        phase: AgentContainmentPhaseV1::VerifyEmpty,
        repeated_failures: 1,
        term_attempts: Vec::new(),
        omitted_term_attempts: 0,
        kill_attempts: Vec::new(),
        omitted_kill_attempts: 0,
        survivors: vec![AgentContainmentProcessV1 {
            pid: 41,
            ppid: 1,
            pgid: 41,
            session_id: 41,
            start_time: 99,
            executable: "/tmp/token=secret-token-sentinel/server".to_string(),
        }],
        omitted_survivors: 2,
    });

    throttle.lifecycle(&context, &observation);
    let events = observer.0.lock().expect("events");
    let ContainmentEvent::CleanupBlocked(event) = &events[0] else {
        panic!("nested blocked event")
    };
    assert_eq!(event.owner.context.worker_id, "worker-nested");
    assert_eq!(event.owner.context.job_id, "job-nested");
    assert_eq!(event.owner.context.attempt_id, "attempt-nested");
    assert_eq!(event.owner.owner_kind, "mcp_server");
    assert_eq!(event.owner.root_pid, Some(41));
    assert_eq!(event.owner.tool_command_id, "[redacted]");
    let encoded = format!("{:?}", events[0]);
    assert!(!encoded.contains("secret-token-sentinel"));
    assert!(!event.survivors.contains("secret-token-sentinel"));
}

#[test]
fn stale_cgroup_failures_are_bounded_warn_evidence() {
    let entries = (0..(MAX_EVENT_SURVIVORS + 5))
        .map(|index| {
            (
                PathBuf::from(format!(
                    "/sys/fs/cgroup/temper/token=secret-token-sentinel/{index}"
                )),
                "authorization: bearer secret-token-sentinel".to_string(),
            )
        })
        .collect::<Vec<_>>();
    let event = startup_scavenge_from_parts(
        "worker-stale",
        3,
        2,
        entries
            .iter()
            .map(|(path, diagnostic)| (path.as_path(), diagnostic.as_str())),
        9,
    )
    .expect("non-empty stale report produces evidence");
    let captured = capture_events(|| event.emit());
    assert_eq!(captured[0]["level"], "WARN");
    let fields = &captured[0]["fields"];
    assert_eq!(fields["event"], "worker.containment.startup_scavenge");
    assert_eq!(fields["worker_id"], "worker-stale");
    assert_eq!(fields["removed_count"], 3);
    assert_eq!(fields["protected_count"], 2);
    assert_eq!(fields["retained_count"], MAX_EVENT_SURVIVORS + 5);
    assert_eq!(fields["omitted_diagnostics"], 14);
    let retained: Vec<Value> =
        serde_json::from_str(fields["retained_diagnostics"].as_str().unwrap()).unwrap();
    assert_eq!(retained.len(), MAX_EVENT_SURVIVORS);
    let encoded = serde_json::to_string(&captured).unwrap();
    assert!(!encoded.contains("secret-token-sentinel"));
    assert!(!encoded.contains("authorization:"));
    assert!(!encoded.contains("bearer "));
}

#[test]
fn live_cgroup_owners_emit_protected_debug_evidence() {
    let event = startup_scavenge_from_parts(
        "worker-concurrent",
        0,
        2,
        std::iter::empty::<(&std::path::Path, &str)>(),
        0,
    )
    .expect("protected owners produce evidence");
    let captured = capture_events(|| event.emit());
    assert_eq!(captured[0]["level"], "DEBUG");
    assert_eq!(captured[0]["fields"]["protected_count"], 2);
    assert_eq!(captured[0]["fields"]["removed_count"], 0);
    assert_eq!(captured[0]["fields"]["retained_count"], 0);
}

#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuffer {
    type Writer = SharedBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn capture_events(run: impl FnOnce()) -> Vec<Value> {
    let buffer = SharedBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buffer.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    let bytes = buffer.0.lock().unwrap().clone();
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
