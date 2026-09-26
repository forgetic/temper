use std::io::Write;
use std::net::Shutdown;
use std::sync::mpsc;
use std::time::Instant;

use temper_protocol_activity::{
    CaptureModeV1, FailureCodeV1, RunStatusV1, ScopeFinishedV1, ScopeStartedV1, ScopeStatusV1,
};
use temper_protocol_worker::FailureClass;

use super::drain_tests::{wait_until, write_event};
use super::*;
use crate::config::WorkerAgentTraceConfig;
use crate::trace::tests::{context, usage_frame};
use crate::trace::{TraceCollector, TraceError};

fn traced_run(capture: CaptureModeV1) -> (tempfile::TempDir, TraceCollector, TraceRun) {
    let temp = tempfile::tempdir().unwrap();
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: temper_protocol_activity::AgentActivityCapturePolicyV1 {
            capture,
            ..Default::default()
        },
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("terminal-completeness", &context())
        .unwrap()
        .unwrap();
    (temp, collector, run)
}

fn main_start() -> AgentActivityEventV1 {
    AgentActivityEventV1::ScopeStarted(ScopeStartedV1 { display_name: None })
}

fn main_finish() -> AgentActivityEventV1 {
    AgentActivityEventV1::ScopeFinished(ScopeFinishedV1 {
        status: ScopeStatusV1::Succeeded,
        duration_ms: 1,
        terminal_reason: None,
    })
}

fn assert_incomplete(run: &TraceRun, collector: &TraceCollector) {
    let error = run
        .finish_success(None)
        .expect_err("missing child boundary is incomplete");
    assert!(matches!(error, TraceError::InvalidSpool(message)
        if message.contains("missing required main-scope boundaries")));
    assert!(
        collector.recover().unwrap()[0]
            .events
            .iter()
            .all(|event| !event.event.is_terminal())
    );
}

#[test]
fn terminal_drain_requires_first_party_boundaries_even_without_any_child_records() {
    for capture in [
        CaptureModeV1::Metadata,
        CaptureModeV1::Transcript,
        CaptureModeV1::Diagnostic,
    ] {
        let (_temp, collector, run) = traced_run(capture);
        let endpoint = run.bind_endpoint_requiring_main_scope().unwrap();
        assert!(endpoint.stop());
        assert_incomplete(&run, &collector);
        run.finish_cancelled()
            .expect("synthetic cancellation remains durable");
        assert!(
            matches!(collector.recover().unwrap()[0].events.last().unwrap().event,
            AgentActivityEventV1::RunFinished(ref end) if end.status == RunStatusV1::Cancelled)
        );
    }
}

#[test]
fn terminal_drain_preserves_legacy_host_only_success() {
    let (_temp, collector, run) = traced_run(CaptureModeV1::Metadata);
    let endpoint = run.bind_endpoint().unwrap();
    assert!(endpoint.stop());
    run.finish_success(None)
        .expect("third-party host-only capture is supported");
    assert_eq!(collector.recover().unwrap()[0].events.len(), 2);
}

#[test]
fn terminal_drain_preserves_optional_legacy_lifecycle_and_disabled_precedence() {
    let (_temp, collector, run) = traced_run(CaptureModeV1::Metadata);
    let mut frame = usage_frame(1);
    frame.event = main_start();
    run.accept_frame(frame).unwrap();
    run.finish_success(None)
        .expect("legacy lifecycle capture remains optional");
    assert_eq!(collector.recover().unwrap()[0].events.len(), 3);

    let (_temp, _collector, run) = traced_run(CaptureModeV1::Metadata);
    let endpoint = run.bind_endpoint_requiring_main_scope().unwrap();
    assert!(endpoint.stop());
    run.inner.state.lock().unwrap().disabled = true;
    assert!(matches!(
        run.finish_success(None),
        Err(TraceError::Disabled)
    ));
}

#[test]
fn terminal_drain_requires_start_even_if_a_finish_is_received() {
    let (_temp, collector, run) = traced_run(CaptureModeV1::Metadata);
    let endpoint = run.bind_endpoint_requiring_main_scope().unwrap();
    let mut frame = usage_frame(1);
    frame.event = main_finish();
    run.accept_frame(frame).unwrap();
    assert!(endpoint.stop());
    assert_incomplete(&run, &collector);
}

#[test]
fn terminal_drain_eof_without_scope_finish_is_incomplete() {
    let (_temp, collector, run) = traced_run(CaptureModeV1::Metadata);
    let endpoint = run.bind_endpoint_requiring_main_scope().unwrap();
    let state = endpoint.state.clone();
    let mut peer = TcpStream::connect(endpoint.address()).unwrap();
    write_event(&mut peer, 0, main_start());
    assert!(wait_until(&state.main_scope_started));
    peer.shutdown(Shutdown::Write).unwrap();
    assert!(wait_until(&state.stream_finished));
    assert!(endpoint.stop());
    assert_incomplete(&run, &collector);
    run.finish_failure(FailureCodeV1::ChildProcess, FailureClass::Transient)
        .expect("crashed producer retains its host failure terminal");
}

#[test]
fn terminal_drain_partial_live_peer_stops_and_keeps_cancellation_terminal() {
    let (_temp, collector, run) = traced_run(CaptureModeV1::Metadata);
    let endpoint = run.bind_endpoint_requiring_main_scope().unwrap();
    let state = endpoint.state.clone();
    let mut peer = TcpStream::connect(endpoint.address()).unwrap();
    write_event(&mut peer, 0, main_start());
    assert!(wait_until(&state.main_scope_started));
    peer.write_all(b"{\"version\":").unwrap();
    let start = Instant::now();
    assert!(endpoint.stop());
    assert!(
        start.elapsed() < FRAME_READ_TIMEOUT + Duration::from_secs(1),
        "live partial peer cannot hold shutdown"
    );
    assert_incomplete(&run, &collector);
    run.finish_cancelled()
        .expect("cancellation is independent of missing child tail");
}

#[test]
fn terminal_drain_wire_budget_bounds_a_peer_that_keeps_its_socket_open() {
    let (_temp, collector, run) = traced_run(CaptureModeV1::Metadata);
    let mut start = usage_frame(1);
    start.event = main_start();
    run.accept_frame(start).unwrap();
    run.inner.state.lock().unwrap().main_scope.require();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (stream, _) = listener.accept().unwrap();
    write_event(
        &mut peer,
        1,
        AgentActivityEventV1::Usage(temper_protocol_activity::UsageV1 {
            input_tokens: 1,
            output_tokens: 1,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }),
    );
    let state = ActivityEndpointState::default();
    state.stopping.store(true, Ordering::Release);
    let began = Instant::now();
    let error = receive_activity_stream(stream, &run, &state, Duration::from_millis(20), 32)
        .expect_err("finite wire budget must reject unread tail");
    assert!(error.contains("terminal activity drain byte limit exhausted"));
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_incomplete(&run, &collector);
}

#[test]
fn terminal_drain_accepts_a_producer_queued_before_receiver_shutdown() {
    let (_temp, collector, run) = traced_run(CaptureModeV1::Metadata);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let state = ActivityEndpointState::default();
    let (release, gate) = mpsc::channel();
    let receiver = {
        let run = run.clone();
        let state = state.clone();
        let address = address.clone();
        thread::spawn(move || {
            gate.recv_timeout(Duration::from_secs(5))
                .expect("release queued consumer");
            serve(listener, run, state, &address, Duration::from_millis(20));
        })
    };
    let endpoint = ActivityEndpoint {
        address,
        state: state.clone(),
        thread: Some(receiver),
    };
    let mut peer = TcpStream::connect(endpoint.address()).unwrap();
    write_event(&mut peer, 0, main_start());
    write_event(&mut peer, 1, main_finish());
    peer.shutdown(Shutdown::Write).unwrap();
    let stopped = thread::spawn(move || endpoint.stop());
    let stop_requested = wait_until(&state.stopping);
    release.send(()).unwrap();
    assert!(stop_requested);
    assert!(stopped.join().unwrap());
    run.finish_success(None).unwrap();
    assert_eq!(
        collector.recover().unwrap()[0]
            .events
            .iter()
            .map(|event| event.event.event_type())
            .collect::<Vec<_>>(),
        vec![
            "run.started",
            "scope.started",
            "scope.finished",
            "run.finished"
        ]
    );
}
