// SPDX-License-Identifier: MPL-2.0

//! B2: the in-process worker→daemon transport delivers worker-protocol messages
//! straight to a co-resident `Daemon` and gets the identical replies an HTTP
//! worker would — register, then poll an enqueued job into an `Assign` — with no
//! socket and no HTTP byte round-trip.

#![cfg(feature = "unified")]

use std::sync::Arc;

use temper_daemon::Daemon;
use temper_worker_orchestrator::Transport;
use temper_worker_protocol::{
    Artifact, Capability, Capacity, Poll, Register, WorkerProtocolMessage,
    WORKER_PROTOCOL_VERSION,
};

use temper::run::InProcessTransport;

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
    temper_io_engine::block_on_with(|cx, handle| async move {
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
        match reply {
            Some(WorkerProtocolMessage::Assign(assign)) => {
                assert_eq!(assign.job_id, "job-1");
                assert_eq!(assign.role, "engineer");
                assert_eq!(assign.repo, "acme/service");
            }
            other => panic!("expected an Assign reply, got {other:?}"),
        }
    });
}
