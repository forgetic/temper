use std::io::Write;
use std::net::Shutdown;
use std::time::Instant;

use temper_protocol_activity::{
    ScopeFinishedV1, ScopeStartedV1, ScopeStatusV1, StopReasonV1, ToolFinishedV1, ToolStartedV1,
    ToolStatusV1, TurnFinishedV1, TurnStartedV1,
};

use super::*;
use crate::config::WorkerAgentTraceConfig;
use crate::trace::TraceCollector;
use crate::trace::tests::{context, usage_frame};

pub(super) fn write_event(stream: &mut TcpStream, turn: u32, event: AgentActivityEventV1) {
    let mut frame = usage_frame(1);
    frame.turn = Some(turn);
    frame.event = event;
    let mut bytes = serde_json::to_vec(&frame).expect("encode frame");
    if matches!(frame.event, AgentActivityEventV1::ScopeFinished(_)) {
        // Exceed the receiver's 8 KiB BufReader capacity so shutdown must drain
        // kernel socket bytes as well as an already buffered prefix.
        bytes.splice(0..0, std::iter::repeat_n(b' ', 16 * 1024));
    }
    bytes.push(b'\n');
    stream.write_all(&bytes).expect("queue frame");
}

pub(super) fn wait_until(flag: &AtomicBool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if flag.load(Ordering::Acquire) {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    false
}

#[test]
fn terminal_drain_preserves_socket_tail_after_blocked_durable_consumer() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: Default::default(),
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("delayed-terminal-drain", &context())
        .unwrap()
        .unwrap();
    let endpoint = ActivityEndpoint::bind_with_read_timeout(run.clone(), Duration::from_millis(20))
        .expect("endpoint");
    let state = endpoint.state.clone();
    let mut stream = TcpStream::connect(endpoint.address()).expect("connect");
    write_event(
        &mut stream,
        14,
        AgentActivityEventV1::ScopeStarted(ScopeStartedV1 { display_name: None }),
    );
    assert!(wait_until(&state.main_scope_started), "durable main start");

    // The real durable-append lock is the consumer gate. No tail record can
    // persist until shutdown has reached the old stopping cutoff.
    let gate = run.inner.state.lock().expect("hold durable consumer");
    for turn in [15, 16] {
        write_event(
            &mut stream,
            turn,
            AgentActivityEventV1::TurnStarted(TurnStartedV1 {}),
        );
        if turn == 15 {
            write_event(
                &mut stream,
                turn,
                AgentActivityEventV1::ToolStarted(ToolStartedV1 {
                    call_id: "submit-tail".to_string(),
                    name: "submit_for_pr".to_string(),
                    arguments: None,
                    shell_discovery_disposition: None,
                    recovery_reference_disposition: None,
                }),
            );
            write_event(
                &mut stream,
                turn,
                AgentActivityEventV1::ToolFinished(ToolFinishedV1 {
                    call_id: "submit-tail".to_string(),
                    name: "submit_for_pr".to_string(),
                    status: ToolStatusV1::Succeeded,
                    duration_ms: 1,
                    result: None,
                    failure: None,
                    codebase_memory_timing: None,
                    graph_correlation: None,
                    decision_anchor_lineage: None,
                    recovery_reference_disposition: None,
                }),
            );
        }
        write_event(
            &mut stream,
            turn,
            AgentActivityEventV1::TurnFinished(TurnFinishedV1 {
                duration_ms: 1,
                stop_reason: StopReasonV1::EndTurn,
            }),
        );
    }
    write_event(
        &mut stream,
        16,
        AgentActivityEventV1::ScopeFinished(ScopeFinishedV1 {
            status: ScopeStatusV1::Succeeded,
            duration_ms: 1,
            terminal_reason: None,
        }),
    );
    stream.shutdown(Shutdown::Write).expect("producer finished");
    let stop = thread::spawn(move || endpoint.stop());
    let stopped = wait_until(&state.stopping);
    drop(gate);
    assert!(stopped, "shutdown reached its finite drain phase");
    assert!(stop.join().expect("endpoint stopper"));
    run.finish_success(None).expect("successful host terminal");
    let recovered = collector.recover().expect("recover durable tail");
    let events = &recovered[0].events;
    assert_eq!(
        events
            .iter()
            .map(|e| e.event.event_type())
            .collect::<Vec<_>>(),
        vec![
            "run.started",
            "scope.started",
            "turn.started",
            "tool.started",
            "tool.finished",
            "turn.finished",
            "turn.started",
            "turn.finished",
            "scope.finished",
            "run.finished",
        ]
    );
    assert_eq!(
        events
            .iter()
            .filter_map(|e| {
                matches!(e.event, AgentActivityEventV1::TurnStarted(_)).then(|| e.turn.unwrap())
            })
            .collect::<Vec<_>>(),
        vec![15, 16]
    );
    assert_eq!(
        events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (1..=10).collect::<Vec<_>>()
    );
}
