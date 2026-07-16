// SPDX-License-Identifier: MPL-2.0

//! Phase 0 spike: the production daemon composition under the lab runtime.
//!
//! One simulated worker registers, long-polls into a timer-driven timeout,
//! receives an assignment, and submits a result that flows through the
//! applier seam — all over the production h1 byte path, on a virtual clock,
//! with a seeded deterministic schedule. The same seed must reproduce the
//! identical trace fingerprint and observable protocol flow.

use std::sync::Arc;

use serde_json::json;
use temper_engine::{Daemon, InFlightJob, ResultApplier};
use temper_engine_io::{CqSender, channel};
use temper_protocol_worker::{
    Artifact, Branch, Capability, Capacity, ErrorCode, JobResult, Poll, Register, RepoOutcome,
    ResultStatus, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};
use temper_sim::{Sim, SimProtocolClient};

struct RecordingApplier {
    tx: CqSender<(InFlightJob, JobResult)>,
}

#[async_trait::async_trait]
impl ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) -> temper_engine::ApplyOutcome {
        let _ = self.tx.send((job, result));
        temper_engine::ApplyOutcome::Applied
    }
}

fn register(worker_id: &str, role: &str, repo: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: role.to_string(),
            repo: repo.to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        worker_pool: None,
        labels: None,
    })
}

fn poll(worker_id: &str, max_wait_ms: u64) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(max_wait_ms),
    })
}

fn success_result(
    worker_id: &str,
    job_id: &str,
    attempt_id: Option<String>,
) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Result(JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        attempt_id,
        status: ResultStatus::Success,
        repos: vec![RepoOutcome {
            repo: "acme/service".to_string(),
            branch: Branch {
                name: "agent/pr-for-code-7".to_string(),
                head_sha: "feedc0de".to_string(),
            },
        }],
        verdict: None,
        title: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: Some("simulated work".to_string()),
        details: None,
    })
}

fn describe(reply: &Option<WorkerProtocolMessage>) -> String {
    match reply {
        None => "no-content".to_string(),
        Some(WorkerProtocolMessage::Assign(assign)) => {
            format!("assign:{}:{}:{}", assign.job_id, assign.role, assign.repo)
        }
        Some(WorkerProtocolMessage::Error(error)) if error.code == ErrorCode::PollTimeout => {
            "poll-timeout".to_string()
        }
        Some(WorkerProtocolMessage::Release(release)) => {
            format!("release:{}:{:?}", release.job_id, release.disposition)
        }
        Some(other) => format!("other:{other:?}"),
    }
}

/// Runs the whole world once and returns (observable events, trace
/// fingerprint, final virtual time in nanoseconds).
fn run_world(seed: u64) -> (Vec<String>, u64, u64) {
    let mut sim = Sim::new(seed);
    let (apply_tx, mut apply_rx) = channel();
    let daemon = Daemon::with_applier(
        sim.engine_spawner(),
        Arc::new(RecordingApplier { tx: apply_tx }),
    );
    let client = SimProtocolClient::new(sim.spawner(), &daemon);

    let events = sim.run_scenario(1_000_000, move |_cx| async move {
        let mut events = Vec::new();

        let reply = client
            .send(&register("sim-worker", "engineer", "acme/service"))
            .await;
        events.push(format!("register => {}", describe(&reply)));

        // Empty queue: the daemon holds the poll and arms a real engine timer;
        // the virtual clock auto-advances to fire it.
        let reply = client.send(&poll("sim-worker", 50)).await;
        events.push(format!("empty poll => {}", describe(&reply)));

        daemon
            .enqueue_job(
                "acme/service/issue-7/engineer/code_ready",
                "engineer",
                "acme/service",
                Artifact {
                    item: json!(7),
                    kind: "issue".to_string(),
                },
                json!({"prompt": "implement", "issue": 7}),
            )
            .await;

        let reply = client.send(&poll("sim-worker", 50)).await;
        events.push(format!("poll => {}", describe(&reply)));
        let assignment = match reply {
            Some(WorkerProtocolMessage::Assign(assign)) => assign,
            other => panic!("expected assignment, got {other:?}"),
        };

        let reply = client
            .send(&success_result(
                "sim-worker",
                &assignment.job_id,
                assignment.attempt_id,
            ))
            .await;
        events.push(format!("result => {}", describe(&reply)));

        let (job, result) = apply_rx.recv().await.expect("applier invoked");
        events.push(format!("applied => {}:{:?}", job.job_id, result.status));

        events
    });

    let report = sim.report();
    (events, report.trace_fingerprint, report.now_nanos)
}

#[test]
fn daemon_protocol_flow_completes_under_simulation() {
    let (events, _fingerprint, now_nanos) = run_world(42);
    assert_eq!(
        events,
        vec![
            "register => no-content",
            "empty poll => poll-timeout",
            "poll => assign:acme/service/issue-7/engineer/code_ready:engineer:acme/service",
            "result => release:acme/service/issue-7/engineer/code_ready:Accepted",
            "applied => acme/service/issue-7/engineer/code_ready:Success",
        ]
    );
    // The empty long-poll waited on the virtual clock (≥50ms of virtual
    // time), not on wall time.
    assert!(
        now_nanos >= 50_000_000,
        "long-poll should advance virtual time past its deadline, got {now_nanos}ns"
    );
}

#[test]
fn same_seed_reproduces_schedule_and_observables() {
    let (events_a, fingerprint_a, now_a) = run_world(42);
    let (events_b, fingerprint_b, now_b) = run_world(42);
    assert_eq!(events_a, events_b);
    assert_eq!(fingerprint_a, fingerprint_b);
    assert_eq!(now_a, now_b);
}

#[test]
fn different_seed_still_satisfies_protocol_flow() {
    let (events_a, ..) = run_world(42);
    let (events_b, ..) = run_world(1337);
    // Scheduling differs, observable protocol behavior must not.
    assert_eq!(events_a, events_b);
}
