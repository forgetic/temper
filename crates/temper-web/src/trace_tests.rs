// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::board::BoardEvent;
use crate::feeds::snapshot_source::FixtureSnapshotSource;
use crate::project::lanes::LaneMap;
use crate::server::AppState;
use std::sync::Mutex;
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentAssignmentIdentityV1, AgentScopeKindV1, AgentScopeV1,
    FailureCodeV1, FailureInfoV1, InlineContentV1, ModelCallRetryingV1, OutputDeltaV1, RunFailedV1,
    RunFinishedV1, RunStartedV1, RunStatusV1,
};

const RAW_SNAPSHOT: &str = r#"{
  "workers":{"healthy":1,"total":1},
  "queued":[{"job_id":"j","role":"code","repo":"ai/temper","ref":"ai/temper#312"}],
  "in_flight":[],"role_saturation":[]
}"#;

fn event(seq: u64, payload: AgentActivityEventV1) -> AgentRunEventV1 {
    AgentRunEventV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: "run-1".to_string(),
        seq,
        occurred_at: "2026-01-01T00:00:00Z".to_string(),
        elapsed_ms: seq,
        assignment: AgentAssignmentIdentityV1 {
            trace_context: None,
            job_id: "j".to_string(),
            repository: "ai/temper".to_string(),
            artifact_ref: "ai/temper#312".to_string(),
            role: "code".to_string(),
            action: "open_pr".to_string(),
            correlation_key: "work".to_string(),
        },
        agent_session_id: None,
        scope: AgentScopeV1 {
            id: "main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: Some(1),
        event: payload,
    }
}

fn summary(last_seq: u64) -> TraceRunSummary {
    TraceRunSummary {
        version: 1,
        run_id: "run-1".to_string(),
        identity: TraceRunIdentity {
            worker_id: "worker".to_string(),
            assignment_id: "assignment".to_string(),
            job_id: "j".to_string(),
            repository: "ai/temper".to_string(),
            artifact_ref: "ai/temper#312".to_string(),
            role: "code".to_string(),
            action: "open_pr".to_string(),
            correlation_key: "work".to_string(),
            agent_session_id: None,
        },
        status: TraceRunStatus::Succeeded,
        started_at: Some("2026-01-01T00:00:00Z".to_string()),
        completed_at: Some("2026-01-01T00:00:01Z".to_string()),
        duration_ms: Some(1_000),
        counts: TraceRunCounts::default(),
        usage: UsageV1 {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
        capture_mode: CaptureModeV1::Diagnostic,
        has_truncated_content: false,
        has_trace_gaps: false,
        dropped_events: 0,
        first_seq: Some(1),
        last_seq,
    }
}

struct FakeClient {
    events: Mutex<Vec<AgentRunEventV1>>,
    event_calls: Mutex<Vec<u64>>,
    artifact_filters: Mutex<Vec<Option<String>>>,
}

impl FakeClient {
    fn new(events: Vec<AgentRunEventV1>) -> Self {
        Self {
            events: Mutex::new(events),
            event_calls: Mutex::new(Vec::new()),
            artifact_filters: Mutex::new(Vec::new()),
        }
    }
}

impl TraceApiClient for FakeClient {
    fn list_runs(
        &self,
        artifact_ref: Option<&str>,
        _cursor: Option<&str>,
        _limit: usize,
    ) -> Result<TraceRunPage, TraceClientError> {
        self.artifact_filters
            .lock()
            .expect("artifact filters")
            .push(artifact_ref.map(str::to_string));
        Ok(TraceRunPage {
            runs: vec![summary(self.events.lock().expect("events").len() as u64)],
            next_cursor: None,
        })
    }

    fn run_summary(&self, _run_id: &str) -> Result<TraceRunSummary, TraceClientError> {
        Ok(summary(self.events.lock().expect("events").len() as u64))
    }

    fn events(
        &self,
        _run_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<TraceEventPage, TraceClientError> {
        self.event_calls.lock().expect("calls").push(after_seq);
        let all = self.events.lock().expect("events");
        let available = all
            .iter()
            .filter(|event| event.seq > after_seq)
            .cloned()
            .collect::<Vec<_>>();
        let has_more = available.len() > limit;
        let events = available.into_iter().take(limit).collect::<Vec<_>>();
        let next_after_seq = events.last().map_or(after_seq, |event| event.seq);
        Ok(TraceEventPage {
            run_id: "run-1".to_string(),
            events,
            next_after_seq,
            has_more,
        })
    }
}

#[test]
fn high_volume_deltas_do_not_flood_global_board_and_cursor_prevents_duplicates() {
    let mut events = vec![event(
        1,
        AgentActivityEventV1::RunStarted(RunStartedV1 {
            capture: CaptureModeV1::Diagnostic,
        }),
    )];
    for seq in 2..=1_002 {
        events.push(event(
            seq,
            AgentActivityEventV1::OutputTextDelta(OutputDeltaV1 {
                delta: InlineContentV1 {
                    text: "x".repeat(32),
                    truncated: false,
                },
            }),
        ));
    }
    events.push(event(
        1_003,
        AgentActivityEventV1::RunFinished(RunFinishedV1 {
            status: RunStatusV1::Succeeded,
            duration_ms: 1_000,
            stop_reason: None,
        }),
    ));
    let client = FakeClient::new(events);
    let state = AppState::new(
        &FixtureSnapshotSource::new(RAW_SNAPSHOT),
        &LaneMap::empty(),
        0,
        std::path::PathBuf::from("/nonexistent"),
    );
    let subscription = state.broadcaster().subscribe();
    let mut poller = TraceActivityPoller::new();

    assert_eq!(poller.poll_once(&state, &client).expect("poll"), 2);
    let frames = std::iter::from_fn(|| subscription.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(frames.len(), 2, "only run boundaries reach board clients");
    for frame in frames {
        let json = frame.trim_start_matches("data: ").trim();
        assert!(matches!(
            serde_json::from_str::<BoardEvent>(json).expect("board event"),
            BoardEvent::CardStream { .. }
        ));
    }

    assert_eq!(poller.poll_once(&state, &client).expect("resume"), 0);
    assert!(
        subscription.try_recv().is_err(),
        "resume emits no duplicates"
    );
    assert_eq!(
        client.event_calls.lock().expect("calls").as_slice(),
        &[0, 500, 1_000, 1_003]
    );
    assert_eq!(
        client
            .artifact_filters
            .lock()
            .expect("artifact filters")
            .as_slice(),
        &[
            Some("ai/temper#312".to_string()),
            Some("ai/temper#312".to_string())
        ],
        "global polling queries only artifacts represented on the board"
    );
}

struct FlakyClient {
    inner: FakeClient,
    fail_next_events: std::sync::atomic::AtomicBool,
}

impl TraceApiClient for FlakyClient {
    fn list_runs(
        &self,
        artifact_ref: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<TraceRunPage, TraceClientError> {
        self.inner.list_runs(artifact_ref, cursor, limit)
    }

    fn run_summary(&self, run_id: &str) -> Result<TraceRunSummary, TraceClientError> {
        self.inner.run_summary(run_id)
    }

    fn events(
        &self,
        run_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<TraceEventPage, TraceClientError> {
        if self
            .fail_next_events
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(TraceClientError::new("engine unavailable"));
        }
        self.inner.events(run_id, after_seq, limit)
    }
}

#[test]
fn engine_outage_retains_cursor_and_the_next_poll_recovers() {
    let events = vec![
        event(
            1,
            AgentActivityEventV1::RunStarted(RunStartedV1 {
                capture: CaptureModeV1::Metadata,
            }),
        ),
        event(
            2,
            AgentActivityEventV1::RunFinished(RunFinishedV1 {
                status: RunStatusV1::Succeeded,
                duration_ms: 10,
                stop_reason: None,
            }),
        ),
    ];
    let client = FlakyClient {
        inner: FakeClient::new(events),
        fail_next_events: std::sync::atomic::AtomicBool::new(true),
    };
    let state = AppState::new(
        &FixtureSnapshotSource::new(RAW_SNAPSHOT),
        &LaneMap::empty(),
        0,
        std::path::PathBuf::from("/nonexistent"),
    );
    let subscription = state.broadcaster().subscribe();
    let mut poller = TraceActivityPoller::new();
    assert!(poller.poll_once(&state, &client).is_err());
    assert!(subscription.try_recv().is_err());
    assert_eq!(poller.poll_once(&state, &client).expect("recovery"), 2);
    assert!(subscription.try_recv().is_ok());
    assert!(subscription.try_recv().is_ok());
}

#[test]
fn transcript_content_and_failure_messages_have_no_global_projection() {
    let delta = event(
        2,
        AgentActivityEventV1::OutputThinkingDelta(OutputDeltaV1 {
            delta: InlineContentV1 {
                text: "private thought".to_string(),
                truncated: false,
            },
        }),
    );
    assert!(board_projection(&delta).is_none());

    let failure = FailureInfoV1 {
        code: FailureCodeV1::Provider,
        message: "secret provider response".to_string(),
        retryable: true,
    };
    let retry = event(
        3,
        AgentActivityEventV1::ModelCallRetrying(ModelCallRetryingV1 {
            call_id: "model-1".to_string(),
            next_attempt: 2,
            delay_ms: 100,
            failure: failure.clone(),
        }),
    );
    let terminal = event(4, AgentActivityEventV1::RunFailed(RunFailedV1 { failure }));
    for projection in [board_projection(&retry), board_projection(&terminal)] {
        let projection = projection.expect("boundary is projected");
        assert!(!projection.v.contains("secret provider response"));
    }
}
