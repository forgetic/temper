// SPDX-License-Identifier: MPL-2.0

//! Reusable whole-world scenarios for sim tests and the CI seed sweep.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use skein::lab::LabConfig;
use skein::lab::chaos::ChaosConfig;
use temper_daemon::Daemon;
use temper_worker_protocol::Artifact;

use crate::model::{ModelApplier, ModelState, SimModel};
use crate::worker::{DoneGuard, WorkerProfile, run_worker};
use crate::{Sim, SimProtocolClient};

pub const MAX_STEPS: u64 = 2_000_000;
pub const ROLE: &str = "engineer";
pub const REPO: &str = "acme/service";

pub struct WorldOutcome {
    pub model: ModelState,
    pub fingerprint: u64,
    pub invariant_violations: Vec<String>,
}

/// A fleet of `count` well-behaved workers.
pub fn fleet(count: usize) -> Vec<WorkerProfile> {
    (0..count)
        .map(|index| WorkerProfile::normal(format!("worker-{index}"), ROLE, REPO))
        .collect()
}

/// Run one drain-the-queue world: `jobs` jobs enqueued by a coordinator
/// task, drained by `workers` simulated workers through the production
/// daemon over real h1 bytes, applies taking `apply_time` of virtual time.
pub fn run_drain_world(
    seed: u64,
    chaos: Option<ChaosConfig>,
    workers: Vec<WorkerProfile>,
    jobs: usize,
    apply_time: Duration,
) -> WorldOutcome {
    let mut config = LabConfig::new(seed);
    if let Some(chaos) = chaos {
        config = config.with_chaos(chaos);
    }
    let mut sim = Sim::with_config(config);

    let model = SimModel::default();
    let daemon = Daemon::with_applier(
        sim.engine_spawner(),
        Arc::new(ModelApplier {
            model: model.clone(),
            spawner: sim.engine_spawner(),
            apply_time,
        }),
    )
    .with_apply_grace(Duration::from_millis(200));

    // Coordinator: enqueue the batch (recording each before handing it over).
    let coordinator_model = model.clone();
    let coordinator_daemon = daemon.clone();
    sim.spawner().spawn_with_cx(move |_cx| async move {
        for index in 0..jobs {
            let job_id = format!("{REPO}/issue-{index}/{ROLE}/code_ready");
            coordinator_model.record_enqueue(&job_id);
            coordinator_daemon
                .enqueue_job(
                    &job_id,
                    ROLE,
                    REPO,
                    Artifact {
                        item: json!(index),
                        kind: "issue".to_string(),
                    },
                    json!({"issue": index}),
                )
                .await;
        }
    });

    // The fleet. Done-guards fire on completion AND on chaos cancellation.
    let finished = Arc::new(AtomicUsize::new(0));
    let fleet_size = workers.len();
    for profile in workers {
        let client = SimProtocolClient::new(sim.spawner(), &daemon);
        let guard = DoneGuard::new(Arc::clone(&finished));
        let worker_model = model.clone();
        sim.spawner().spawn_with_cx(move |cx| async move {
            run_worker(cx, client, worker_model, profile, guard).await;
        });
    }

    sim.run_until(|| finished.load(Ordering::SeqCst) == fleet_size, MAX_STEPS);
    sim.settle(MAX_STEPS);

    let report = sim.report();
    WorldOutcome {
        model: model.snapshot(),
        fingerprint: report.trace_fingerprint,
        invariant_violations: report.invariant_violations,
    }
}

/// The cancel-chaos configuration used by the regression seed corpus: kills
/// workers, connections, and daemon tasks mid-flight.
pub fn cancel_chaos(seed: u64) -> ChaosConfig {
    ChaosConfig {
        seed,
        cancel_probability: 0.02,
        delay_probability: 0.10,
        delay_range: Duration::ZERO..Duration::from_millis(50),
        wakeup_storm_probability: 0.02,
        wakeup_storm_count: 1..10,
        budget_exhaust_probability: 0.01,
        ..ChaosConfig::off()
    }
}
