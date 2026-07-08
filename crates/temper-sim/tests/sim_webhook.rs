// SPDX-License-Identifier: MPL-2.0

//! Phase 1: webhook storms under the deterministic sim.
//!
//! A burst of concurrent (and replayed) webhook deliveries hits the daemon's
//! production webhook route over real h1 bytes. Every delivery must be
//! answered (no responder leaks behind the held-202 wake-scan window), the
//! discovered work must be enqueued and dispatched exactly once, and the
//! whole world must reproduce per seed.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;
use temper_engine::{Daemon, RoleFeedMode, RoleFeedTarget, WebhookConfig, webhook_signature};
use temper_forge::{CreateIssue, CreateRepository, Forge, ItemNumber, RepositoryId, UserId};
use temper_forge_memory::MemoryForge;
use temper_protocol_worker::{
    Capability, Capacity, ErrorCode, Poll, Register, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};
use temper_sim::{Sim, SimProtocolClient, http_request};
use temper_workflow::{RawWorkflowSpec, RoleId, ValidatedWorkflow};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");
const MAX_STEPS: u64 = 2_000_000;
const SECRET: &str = "s3cret";

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn webhook_request(issue: ItemNumber) -> skein::http::h1::Request {
    let body = serde_json::to_vec(&json!({
        "repository": { "full_name": "acme/service" },
        "issue": { "number": issue.get() }
    }))
    .expect("webhook body serializes");
    let signature = webhook_signature(SECRET, &body);
    skein::http::h1::Request {
        method: skein::http::h1::Method::Post,
        uri: "/forgejo/webhook".to_string(),
        version: skein::http::h1::Version::Http11,
        headers: vec![
            ("x-forgejo-event".to_string(), "issues".to_string()),
            (
                "x-forgejo-signature".to_string(),
                format!("sha256={signature}"),
            ),
            ("content-type".to_string(), "application/json".to_string()),
        ],
        body,
        trailers: Vec::new(),
        peer_addr: None,
    }
}

fn register_message(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: "engineer".to_string(),
            repo: "acme/service".to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        worker_pool: None,
        labels: None,
    })
}

fn poll_message(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(20),
    })
}

/// Build a world with one ready issue behind the webhook route; deliver
/// `storm` concurrent replayed webhooks; then drain assignments with one
/// worker. Returns (delivery statuses, assignment count, fingerprint).
fn run_world(seed: u64, storm: usize) -> (Vec<u16>, usize, u64) {
    let mut sim = Sim::new(seed);

    let forge = Arc::new(MemoryForge::new());
    let workflow = Arc::new(workflow());
    let compiled = Arc::new(workflow.compile());

    // Seed the forge (pure in-memory, no runtime needed — but creation is
    // async, so run it as a scenario first).
    let setup_forge = Arc::clone(&forge);
    let (repo, issue) = sim.run_scenario(MAX_STEPS, move |_cx| async move {
        let repo: RepositoryId = setup_forge
            .create_repository(CreateRepository {
                owner: "acme".to_string(),
                name: "service".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id;
        let issue = setup_forge
            .create_issue(
                &repo,
                CreateIssue {
                    title: "ready code issue".to_string(),
                    body: "Implement the webhook storm scenario.".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    assignees: Vec::<UserId>::new(),
                },
            )
            .await
            .expect("issue is created")
            .number;
        (repo, issue)
    });

    let daemon = Daemon::new(sim.engine_spawner()).with_webhook(
        forge,
        workflow,
        compiled,
        Arc::new(WebhookConfig {
            secret: SECRET.to_string(),
            targets: vec![RoleFeedTarget {
                repo,
                role: RoleId::new("engineer"),
                mode: RoleFeedMode::Wake,
            }],
        }),
        sim.wall_clock(),
    );
    let handler = temper_engine::h1_handler(&daemon);

    // The storm: `storm` concurrent deliveries of the same event.
    let statuses: Arc<Mutex<Vec<u16>>> = Arc::new(Mutex::new(Vec::new()));
    let delivered = Arc::new(AtomicUsize::new(0));
    for _ in 0..storm {
        let spawner = sim.spawner();
        let handler = Arc::clone(&handler);
        let statuses = Arc::clone(&statuses);
        let delivered = Arc::clone(&delivered);
        sim.spawner().spawn_with_cx(move |_cx| async move {
            let response = http_request(&spawner, &handler, webhook_request(issue))
                .await
                .expect("webhook delivery is answered");
            statuses.lock().expect("statuses").push(response.status);
            delivered.fetch_add(1, Ordering::SeqCst);
        });
    }
    sim.run_until(|| delivered.load(Ordering::SeqCst) == storm, MAX_STEPS);

    // One worker drains whatever the wake scans enqueued.
    let client = SimProtocolClient::new(sim.spawner(), &daemon);
    let assignments = sim.run_scenario(MAX_STEPS, move |_cx| async move {
        client.send(&register_message("worker-a")).await;
        let mut assignments = 0usize;
        let mut empty = 0u32;
        while empty < 3 {
            match client.send(&poll_message("worker-a")).await {
                Some(WorkerProtocolMessage::Assign(_)) => assignments += 1,
                Some(WorkerProtocolMessage::Error(error))
                    if error.code == ErrorCode::PollTimeout =>
                {
                    empty += 1;
                }
                other => panic!("unexpected poll reply: {other:?}"),
            }
        }
        assignments
    });
    sim.settle(MAX_STEPS);

    let statuses = {
        let mut statuses = statuses.lock().expect("statuses").clone();
        statuses.sort_unstable();
        statuses
    };
    (statuses, assignments, sim.report().trace_fingerprint)
}

#[test]
fn webhook_storm_is_fully_answered_and_enqueues_once() {
    let (statuses, assignments, _fingerprint) = run_world(42, 6);
    // Every delivery answered with 202, and the replayed event produced exactly
    // one dispatchable job.
    assert_eq!(statuses, vec![202; 6], "every delivery is answered 202");
    assert_eq!(
        assignments, 1,
        "replayed deliveries must not duplicate work"
    );
}

#[test]
fn webhook_storm_is_deterministic_per_seed() {
    let a = run_world(7, 6);
    let b = run_world(7, 6);
    assert_eq!(a, b);
}
