// SPDX-License-Identifier: MPL-2.0

use std::io::{BufRead, BufReader};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityEventV1, AgentAssignmentIdentityV1, AgentRunEventV1,
    AgentScopeKindV1, AgentScopeV1, CaptureModeV1, RunStartedV1,
};

use super::*;
use crate::feeds::snapshot_source::EmptySnapshotSource;
use crate::project::lanes::LaneMap;
use crate::trace::{TraceApiClient, TraceEventPage, TraceRunPage, TraceRunSummary};

struct FakeEngine {
    events: Vec<AgentRunEventV1>,
    calls: Mutex<Vec<u64>>,
    fail_first: AtomicBool,
}

impl TraceApiClient for FakeEngine {
    fn list_runs(
        &self,
        _artifact_ref: Option<&str>,
        _cursor: Option<&str>,
        _limit: usize,
    ) -> Result<TraceRunPage, TraceClientError> {
        Err(TraceClientError::new("unused list route"))
    }

    fn run_summary(&self, _run_id: &str) -> Result<TraceRunSummary, TraceClientError> {
        Err(TraceClientError::new("unused summary route"))
    }

    fn events(
        &self,
        _run_id: &str,
        after_seq: u64,
        _limit: usize,
    ) -> Result<TraceEventPage, TraceClientError> {
        self.calls.lock().expect("calls").push(after_seq);
        if self.fail_first.swap(false, Ordering::SeqCst) {
            return Err(TraceClientError::new("temporary outage"));
        }
        // Deliberately replay the cursor event. The web SSE boundary must still
        // emit strictly newer sequence IDs without duplicates.
        Ok(TraceEventPage {
            run_id: "run-1".to_string(),
            events: self.events.clone(),
            next_after_seq: self.events.last().map_or(after_seq, |event| event.seq),
            has_more: false,
        })
    }
}

fn event(seq: u64) -> AgentRunEventV1 {
    AgentRunEventV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: "run-1".to_string(),
        seq,
        occurred_at: "2026-01-01T00:00:00Z".to_string(),
        elapsed_ms: seq,
        assignment: AgentAssignmentIdentityV1 {
            job_id: "job".to_string(),
            repository: "ai/temper".to_string(),
            artifact_ref: "ai/temper#312".to_string(),
            role: "engineer".to_string(),
            action: "open_pr".to_string(),
            correlation_key: "work".to_string(),
        },
        agent_session_id: None,
        scope: AgentScopeV1 {
            id: "main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: None,
        event: AgentActivityEventV1::RunStarted(RunStartedV1 {
            capture: CaptureModeV1::Metadata,
        }),
    }
}

#[test]
fn stream_target_decodes_run_id_and_resume_cursor() {
    assert_eq!(
        parse_trace_stream_target("/api/agent-runs/run%2Fpart/stream?after_seq=41"),
        Ok(Some(("run/part".to_string(), 41)))
    );
    assert!(parse_trace_stream_target("/api/agent-runs/run/stream?after_seq=bad").is_err());
    assert_eq!(parse_trace_stream_target("/api/state"), Ok(None));
}

#[test]
fn detailed_sse_recovers_from_outage_resumes_without_duplicates_and_stops_on_close() {
    let engine = Arc::new(FakeEngine {
        events: vec![event(2), event(3), event(4)],
        calls: Mutex::new(Vec::new()),
        fail_first: AtomicBool::new(true),
    });
    let state = Arc::new(
        AppState::new(
            &EmptySnapshotSource,
            &LaneMap::empty(),
            0,
            std::path::PathBuf::from("/nonexistent"),
        )
        .with_trace_client(engine.clone()),
    );

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind browser socket");
    let browser = TcpStream::connect(listener.local_addr().expect("address")).expect("connect");
    browser
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    let (server, _) = listener.accept().expect("accept");
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        done_tx
            .send(serve_trace_events(
                server,
                &state,
                "run-1",
                2,
                Duration::from_millis(10),
            ))
            .ok();
    });

    let mut reader = BufReader::new(browser);
    let mut wire = String::new();
    while !wire.contains("id: 4\n") {
        let mut line = String::new();
        let read = reader.read_line(&mut line).expect("SSE line");
        assert!(read > 0, "SSE closed before resumed events arrived");
        wire.push_str(&line);
    }
    assert!(!wire.contains("id: 2\n"), "cursor event must not replay");
    assert_eq!(wire.matches("id: 3\n").count(), 1);
    assert_eq!(wire.matches("id: 4\n").count(), 1);

    reader
        .get_ref()
        .shutdown(Shutdown::Both)
        .expect("close browser");
    assert!(
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("SSE worker stops after disconnect")
            .is_err(),
        "closed socket ends the detailed polling worker"
    );

    let calls = engine.calls.lock().expect("calls").clone();
    assert!(calls.len() >= 2);
    assert_eq!(&calls[..2], &[2, 2], "outage retry retains resume cursor");
    let stopped_at = calls.len();
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(engine.calls.lock().expect("calls").len(), stopped_at);
}
