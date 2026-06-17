// SPDX-License-Identifier: MPL-2.0

//! B2: the in-process worker→daemon transport delivers worker-protocol messages
//! straight to a co-resident `Daemon` and gets the identical replies an HTTP
//! worker would — register, then poll an enqueued job into an `Assign` — with no
//! socket and no HTTP byte round-trip.

use std::sync::Arc;

use temper_cli::InProcessTransport;
use temper_engine::Daemon;
use temper_protocol_worker::{
    Artifact, Branch, Capability, Capacity, JobResult, Poll, Register, RepoOutcome, ResultStatus,
    WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};
use temper_worker::Transport;

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
        labels: None,
    })
}

fn poll(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        // No long wait: the job is already enqueued, so the first poll assigns.
        max_wait_ms: Some(0),
    })
}

#[test]
fn in_process_transport_registers_polls_and_assigns() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let daemon = Daemon::new(Arc::new(handle.clone()));
        let transport = InProcessTransport::new(daemon.clone());

        // Register the worker in-process.
        let registered = transport
            .send(cx.clone(), register("w1", "engineer", "acme/service"))
            .await;
        assert!(
            registered.is_ok(),
            "register should succeed in-process: {registered:?}"
        );

        // Enqueue one job for the worker's (role, repo).
        daemon
            .enqueue_job(
                "job-1",
                "engineer",
                "acme/service",
                Artifact {
                    item: serde_json::json!(42),
                    kind: "issue".to_string(),
                },
                serde_json::json!({}),
            )
            .await;

        // Poll: the enqueued job comes back as an Assign over the in-process
        // carrier — exactly as it would over HTTP.
        let reply = transport
            .send(cx.clone(), poll("w1"))
            .await
            .expect("poll succeeds in-process");
        let job_id = match reply {
            Some(WorkerProtocolMessage::Assign(assign)) => {
                assert_eq!(assign.job_id, "job-1");
                assert_eq!(assign.role, "engineer");
                assert_eq!(assign.repo, "acme/service");
                assign.job_id
            }
            other => panic!("expected an Assign reply, got {other:?}"),
        };

        // Deliver the job result in-process — the full register→poll→assign→
        // result lifecycle over the in-memory carrier, no socket.
        let result = transport
            .send(
                cx.clone(),
                WorkerProtocolMessage::Result(JobResult {
                    protocol_version: WORKER_PROTOCOL_VERSION,
                    worker_id: "w1".to_string(),
                    job_id: job_id.clone(),
                    status: ResultStatus::Success,
                    repos: vec![RepoOutcome {
                        repo: "acme/service".to_string(),
                        branch: Branch {
                            name: format!("agent/{job_id}"),
                            head_sha: "feedc0de".to_string(),
                        },
                    }],
                    verdict: None,
                    body: None,
                    children: Vec::new(),
                    failure: None,
                    summary: Some("done".to_string()),
                    details: None,
                }),
            )
            .await;
        assert!(
            result.is_ok(),
            "result delivery should succeed in-process: {result:?}"
        );
    });
}
