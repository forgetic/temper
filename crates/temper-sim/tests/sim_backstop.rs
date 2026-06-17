// SPDX-License-Identifier: MPL-2.0

//! Phase 2: the daemon's poll-backstop cadence loop under virtual time.
//!
//! The production cadence machine (`spawn_poll_backstop` →
//! `spawn_cadence_loop` → `CadenceMachine` + timers) runs under the lab.
//! Work that appears on the forge *after* the first scan must be discovered
//! by a later tick — which only happens because the virtual clock advances
//! through the cadence timer. Wall time plays no part: the injected
//! `WallClock` derives from the same virtual clock.

use std::sync::Arc;

use temper_engine::{
    Daemon, PollBackstopConfig, RoleFeedMode, RoleFeedTarget, spawn_poll_backstop,
};
use temper_forge::{CreateIssue, CreateRepository, Forge, RepositoryId, UserId};
use temper_forge_memory::MemoryForge;
use temper_protocol_worker::{
    Capability, Capacity, ErrorCode, Poll, Register, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};
use temper_sim::{Sim, SimProtocolClient};
use temper_workflow::{RawWorkflowSpec, RoleId, ValidatedWorkflow};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");
const MAX_STEPS: u64 = 4_000_000;
const CADENCE: std::time::Duration = std::time::Duration::from_secs(60);

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
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
        labels: None,
    })
}

fn poll_message(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(100),
    })
}

/// Returns (polls until assignment, virtual nanos at assignment, fingerprint).
fn run_world(seed: u64) -> (u32, u64, u64) {
    let mut sim = Sim::new(seed);

    let forge = Arc::new(MemoryForge::new());
    let workflow = Arc::new(workflow());
    let compiled = Arc::new(workflow.compile());

    let setup_forge = Arc::clone(&forge);
    let repo: RepositoryId = sim.run_scenario(MAX_STEPS, move |_cx| async move {
        setup_forge
            .create_repository(CreateRepository {
                owner: "acme".to_string(),
                name: "service".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id
    });

    let daemon = Daemon::new(sim.engine_spawner());
    spawn_poll_backstop(
        &sim.engine_spawner(),
        daemon.clone(),
        Arc::clone(&forge),
        Arc::clone(&workflow),
        compiled,
        PollBackstopConfig {
            targets: vec![RoleFeedTarget {
                repo: repo.clone(),
                role: RoleId::new("engineer"),
                mode: RoleFeedMode::Normal,
            }],
            cadence: CADENCE,
        },
        sim.wall_clock(),
    );

    // Let the first (empty) scan happen, then create the ready issue: only a
    // LATER cadence tick — one virtual minute out — can discover it.
    let client = SimProtocolClient::new(sim.spawner(), &daemon);
    let scenario_forge = Arc::clone(&forge);
    let polls_until_assign = sim.run_scenario(MAX_STEPS, move |cx| async move {
        client.send(&register_message("worker-a")).await;
        // One virtual second guarantees the immediate first tick has scanned
        // the still-empty repo; only the next cadence tick can discover the
        // issue created below.
        skein::time::sleep(
            temper_engine_io::runtime::timer_now(&cx),
            std::time::Duration::from_secs(1),
        )
        .await;
        scenario_forge
            .create_issue(
                &repo,
                CreateIssue {
                    title: "late-arriving code issue".to_string(),
                    body: "Discovered only by a cadence tick.".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    assignees: Vec::<UserId>::new(),
                },
            )
            .await
            .expect("issue is created");

        let mut polls = 0u32;
        loop {
            polls += 1;
            match client.send(&poll_message("worker-a")).await {
                Some(WorkerProtocolMessage::Assign(_)) => return polls,
                Some(WorkerProtocolMessage::Error(error))
                    if error.code == ErrorCode::PollTimeout =>
                {
                    assert!(polls < 10_000, "assignment never arrived");
                }
                other => panic!("unexpected poll reply: {other:?}"),
            }
        }
    });

    let report = sim.report();
    (polls_until_assign, report.now_nanos, report.fingerprint())
}

trait Fingerprint {
    fn fingerprint(&self) -> u64;
}

impl Fingerprint for skein::lab::LabRunReport {
    fn fingerprint(&self) -> u64 {
        self.trace_fingerprint
    }
}

#[test]
fn cadence_backstop_discovers_late_work_on_virtual_time() {
    let (polls, now_nanos, _fingerprint) = run_world(42);
    assert!(polls > 1, "the first scan ran before the issue existed");
    // Discovery requires at least one full cadence of virtual time; the test
    // itself completes in milliseconds of wall time.
    assert!(
        now_nanos >= CADENCE.as_nanos() as u64,
        "discovery needed a cadence tick: {now_nanos}ns < {:?}",
        CADENCE
    );
}

#[test]
fn cadence_backstop_is_deterministic_per_seed() {
    assert_eq!(run_world(42), run_world(42));
}
