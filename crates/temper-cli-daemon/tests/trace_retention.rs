// SPDX-License-Identifier: MPL-2.0

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_engine::{
    AgentTraceJournal, AuthenticatedWorkerBinding, Daemon, NoopApplier, TraceJournalConfig,
};
use temper_engine_service::spawn_trace_retention_task;
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityBatch, AgentActivityCapturePolicyV1,
    AgentActivityEventV1, AgentAssignmentIdentityV1, AgentRunEventV1, AgentScopeKindV1,
    AgentScopeV1, CaptureModeV1, RunFinishedV1, RunStartedV1, RunStatusV1,
};
use temper_protocol_worker::{
    Artifact, Capability, Capacity, Poll, Register, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

fn timestamp(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid timestamp")
}

fn policy() -> AgentActivityCapturePolicyV1 {
    AgentActivityCapturePolicyV1 {
        capture: CaptureModeV1::Metadata,
        retention_days: 1,
        max_run_bytes: 32_000,
        max_inline_bytes: 128,
        max_blob_bytes: 512,
        capture_thinking: false,
        version: ACTIVITY_PROTOCOL_VERSION,
    }
}

fn binding(job_id: &str, policy: &AgentActivityCapturePolicyV1) -> AuthenticatedWorkerBinding {
    AuthenticatedWorkerBinding {
        worker_id: "worker-retention".to_string(),
        assignment_id: job_id.to_string(),
        assignment: AgentAssignmentIdentityV1 {
            trace_context: None,
            job_id: job_id.to_string(),
            repository: "ai/temper".to_string(),
            artifact_ref: "ai/temper#349".to_string(),
            role: "engineer".to_string(),
            action: "open_pr".to_string(),
            correlation_key: format!("retention-{job_id}"),
        },
        agent_session_id: None,
        capture_policy: policy.clone(),
    }
}

fn terminal_batch(run_id: &str, binding: &AuthenticatedWorkerBinding) -> AgentActivityBatch {
    let event = |seq, event| AgentRunEventV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: run_id.to_string(),
        seq,
        occurred_at: format!("2026-01-01T00:00:0{}Z", seq - 1),
        elapsed_ms: (seq - 1) * 10,
        assignment: binding.assignment.clone(),
        agent_session_id: None,
        scope: AgentScopeV1 {
            id: "main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: None,
        event,
    };
    AgentActivityBatch {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: run_id.to_string(),
        first_seq: 1,
        events: vec![
            event(
                1,
                AgentActivityEventV1::RunStarted(RunStartedV1 {
                    capture: binding.capture_policy.capture,
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
        ],
        blobs: Vec::new(),
    }
}

#[test]
fn periodic_retention_removes_expired_runs_preserves_in_flight_and_stops() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let temporary = tempfile::tempdir().expect("tempdir");
        let clock_value = Arc::new(Mutex::new(timestamp("2026-01-01T00:00:30Z")));
        let clock = {
            let clock_value = Arc::clone(&clock_value);
            Arc::new(move || *clock_value.lock().expect("clock lock")) as temper_engine::WallClock
        };
        let policy = policy();
        let journal = AgentTraceJournal::open_with_clock(
            TraceJournalConfig {
                root: temporary.path().join("journal"),
                policy: policy.clone(),
            },
            clock,
        )
        .expect("journal opens");

        let protected_job = "job-in-flight";
        let protected_binding = binding(protected_job, &policy);
        journal
            .ingest(
                &protected_binding,
                &terminal_batch("run-protected", &protected_binding),
            )
            .expect("protected run ingests");
        let expired_binding = binding("job-complete", &policy);
        journal
            .ingest(
                &expired_binding,
                &terminal_batch("run-expired", &expired_binding),
            )
            .expect("expired run ingests");

        // One corrupt sibling proves a failed cleanup target does not stop the
        // pass from removing an unrelated valid run or kill future cadence.
        std::fs::create_dir(journal.root().join("runs/000-corrupt"))
            .expect("corrupt run directory");

        let daemon = Daemon::with_applier(Arc::new(handle.clone()), Arc::new(NoopApplier));
        daemon
            .deliver_protocol_message(WorkerProtocolMessage::Register(Register {
                protocol_version: WORKER_PROTOCOL_VERSION,
                worker_id: "worker-retention".to_string(),
                worker_pool: None,
                capabilities: vec![Capability {
                    role: "engineer".to_string(),
                    repo: "ai/temper".to_string(),
                }],
                capacity: Capacity {
                    max_concurrent_jobs: 1,
                },
                labels: None,
            }))
            .await
            .expect("register succeeds");
        daemon
            .enqueue_job(
                protected_job,
                "engineer",
                "ai/temper",
                Artifact {
                    item: Default::default(),
                    kind: "issue".to_string(),
                },
                Default::default(),
            )
            .await;
        let assigned = daemon
            .deliver_protocol_message(WorkerProtocolMessage::Poll(Poll {
                protocol_version: WORKER_PROTOCOL_VERSION,
                worker_id: "worker-retention".to_string(),
                free_capacity: 1,
                max_wait_ms: Some(0),
            }))
            .await
            .expect("poll succeeds")
            .expect("assignment reply");
        assert!(matches!(assigned, WorkerProtocolMessage::Assign(_)));

        *clock_value.lock().expect("clock lock") = timestamp("2026-01-04T00:00:30Z");
        let spawner: Arc<dyn temper_engine_io::Spawner> = Arc::new(handle);
        let task =
            spawn_trace_retention_task(&spawner, daemon, journal.clone(), Duration::from_millis(5));
        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(40)).await;

        assert!(
            journal
                .manifest("run-expired")
                .expect("expired lookup")
                .is_none()
        );
        assert!(
            journal
                .manifest("run-protected")
                .expect("protected lookup")
                .is_some()
        );

        task.stop().await;
        let after_stop_binding = binding("job-after-stop", &policy);
        journal
            .ingest(
                &after_stop_binding,
                &terminal_batch("run-after-stop", &after_stop_binding),
            )
            .expect("post-stop run ingests");
        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(20)).await;
        assert!(
            journal
                .manifest("run-after-stop")
                .expect("post-stop lookup")
                .is_some(),
            "a joined retention task must not run after shutdown"
        );
    });
}
