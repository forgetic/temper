// SPDX-License-Identifier: MPL-2.0

//! Real worker harness for simulation.
//!
//! The original sim worker in [`crate::worker`] is intentionally hand-rolled so
//! tests can cheaply exercise protocol misbehavior and the HTTP byte path. This
//! module is the higher-fidelity companion: it spawns the production
//! `temper_worker::run_worker_with_transport` loop (`WorkerMachine` +
//! `WorkerShell`) on the lab spawner, uses `StubExecutor::success()`, and points
//! it at a co-resident [`temper_engine::Daemon`] through the reusable
//! [`crate::InProcessTransport`].

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::json;
use skein::cx::Cx;
use temper_engine::Daemon;
use temper_protocol_worker::{
    Artifact, ReleaseDisposition, ResultStatus, WorkerAuth, WorkerProtocolMessage,
};
use temper_worker::{
    CapabilitySpec, ExecutorSelection, StubExecutor, Transport, WorkerConfig,
    run_worker_with_transport,
};

use crate::model::SimModel;
use crate::{InProcessTransport, Sim};

/// Default real-worker scenario job id used by [`run_success_stub_worker_once`].
pub const REAL_WORKER_JOB_ID: &str = "acme/service/issue-real/engineer/code_ready";

/// Runtime/config profile for a real worker inside simulation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealWorkerProfile {
    pub worker_id: String,
    pub role: String,
    pub repo: String,
    pub max_concurrent_jobs: u32,
    pub poll_wait: Duration,
    pub heartbeat_interval: Duration,
}

impl RealWorkerProfile {
    pub fn new(worker_id: impl Into<String>, role: &str, repo: &str) -> Self {
        Self {
            worker_id: worker_id.into(),
            role: role.to_string(),
            repo: repo.to_string(),
            max_concurrent_jobs: 1,
            poll_wait: Duration::from_millis(50),
            heartbeat_interval: Duration::from_millis(50),
        }
    }

    /// Build the production worker config consumed by `run_worker_with_transport`.
    pub fn worker_config(&self) -> WorkerConfig {
        WorkerConfig {
            daemon_url: "in-process://temper-sim".to_string(),
            worker_id: self.worker_id.clone(),
            worker_pool: None,
            worker_auth: None,
            capabilities: vec![CapabilitySpec {
                repo: self.repo.clone(),
                role: self.role.clone(),
            }],
            role_identities: BTreeMap::new(),
            max_concurrent_jobs: self.max_concurrent_jobs,
            poll_wait: self.poll_wait,
            heartbeat_interval: self.heartbeat_interval,
            executor: ExecutorSelection::Stub,
        }
    }
}

/// Observable protocol milestones from the real worker shell's transport.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RealWorkerTrace {
    pub registers: u64,
    pub polls: u64,
    pub heartbeats: u64,
    pub assignments: Vec<String>,
    pub results: Vec<(String, ResultStatus)>,
    pub releases: Vec<(String, ReleaseDisposition)>,
    pub transport_errors: Vec<String>,
}

impl RealWorkerTrace {
    pub fn assigned(&self, job_id: &str) -> bool {
        self.assignments.iter().any(|seen| seen == job_id)
    }

    pub fn submitted_success(&self, job_id: &str) -> bool {
        self.results
            .iter()
            .any(|(seen, status)| seen == job_id && *status == ResultStatus::Success)
    }

    pub fn released(&self, job_id: &str) -> bool {
        self.releases.iter().any(|(seen, _)| seen == job_id)
    }

    pub fn accepted_release(&self, job_id: &str) -> bool {
        self.releases.iter().any(|(seen, disposition)| {
            seen == job_id && *disposition == ReleaseDisposition::Accepted
        })
    }
}

/// Shared trace handle for a spawned real worker.
#[derive(Clone, Default)]
pub struct RealWorkerProbe(Arc<Mutex<RealWorkerTrace>>);

impl RealWorkerProbe {
    fn trace(&self) -> MutexGuard<'_, RealWorkerTrace> {
        self.0.lock().expect("real-worker probe lock")
    }

    pub fn snapshot(&self) -> RealWorkerTrace {
        self.trace().clone()
    }

    fn record_exchange(
        &self,
        sent: &WorkerProtocolMessage,
        reply: &Result<Option<WorkerProtocolMessage>, String>,
        model: Option<&SimModel>,
    ) {
        let poll_worker = match sent {
            WorkerProtocolMessage::Register(_) => {
                self.trace().registers += 1;
                None
            }
            WorkerProtocolMessage::Poll(poll) => {
                self.trace().polls += 1;
                Some(poll.worker_id.clone())
            }
            WorkerProtocolMessage::Heartbeat(_) => {
                self.trace().heartbeats += 1;
                None
            }
            WorkerProtocolMessage::Result(result) => {
                self.trace()
                    .results
                    .push((result.job_id.clone(), result.status));
                None
            }
            _ => None,
        };

        match reply {
            Ok(Some(WorkerProtocolMessage::Assign(assign))) => {
                self.trace().assignments.push(assign.job_id.clone());
                if let (Some(model), Some(worker_id)) = (model, poll_worker.as_deref()) {
                    model.record_assign(&assign.job_id, worker_id);
                }
            }
            Ok(Some(WorkerProtocolMessage::Release(release))) => {
                self.trace()
                    .releases
                    .push((release.job_id.clone(), release.disposition));
                if let Some(model) = model {
                    model.record_release(&release.job_id);
                }
            }
            Err(error) => self.trace().transport_errors.push(error.clone()),
            Ok(_) => {}
        }
    }
}

/// In-process daemon transport with deterministic observation hooks.
pub struct ObservedInProcessTransport {
    inner: InProcessTransport,
    probe: RealWorkerProbe,
    model: Option<SimModel>,
}

impl ObservedInProcessTransport {
    pub fn new(daemon: Daemon, probe: RealWorkerProbe, model: Option<SimModel>) -> Self {
        Self {
            inner: InProcessTransport::new(daemon),
            probe,
            model,
        }
    }
}

impl Transport for ObservedInProcessTransport {
    fn send(
        &self,
        cx: Cx,
        message: WorkerProtocolMessage,
        auth: Option<WorkerAuth>,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        let inner = self.inner.clone();
        let probe = self.probe.clone();
        let model = self.model.clone();
        async move {
            let reply = Transport::send(&inner, cx, message.clone(), auth).await;
            probe.record_exchange(&message, &reply, model.as_ref());
            reply
        }
    }
}

/// Spawn a production `WorkerMachine`/`WorkerShell` with a success stub executor
/// on the lab runtime. The worker runs like production (forever); tests drive the
/// lab until their model/probe condition is met and then drop the world.
pub fn spawn_success_stub_worker(
    sim: &Sim,
    daemon: &Daemon,
    profile: RealWorkerProfile,
    model: Option<SimModel>,
) -> RealWorkerProbe {
    let probe = RealWorkerProbe::default();
    let transport = Arc::new(ObservedInProcessTransport::new(
        daemon.clone(),
        probe.clone(),
        model,
    ));
    let config = profile.worker_config();
    let executor = Arc::new(StubExecutor::success());
    let spawner = sim.spawner();
    let worker_spawner = spawner.clone();

    spawner.spawn_with_cx(move |_cx| async move {
        let _ = run_worker_with_transport(worker_spawner, config, executor, transport).await;
    });

    probe
}

/// Outcome from the one-job real-worker scenario.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealWorkerOutcome {
    pub model: crate::model::ModelState,
    pub trace: RealWorkerTrace,
}

/// Run one deterministic high-fidelity worker world: enqueue one job, execute it
/// with the real worker shell/machine and `StubExecutor::success()`, and return
/// the observed model/transport trace once the daemon has released and applied
/// the job.
pub fn run_success_stub_worker_once(seed: u64) -> RealWorkerOutcome {
    let mut sim = Sim::new(seed);
    let model = SimModel::default();
    let daemon = Daemon::with_applier(
        sim.engine_spawner(),
        Arc::new(crate::model::ModelApplier {
            model: model.clone(),
            spawner: sim.engine_spawner(),
            apply_time: Duration::ZERO,
        }),
    )
    .with_apply_grace(Duration::from_millis(200));

    let profile = RealWorkerProfile::new("real-worker-0", "engineer", "acme/service");
    let probe = spawn_success_stub_worker(&sim, &daemon, profile, Some(model.clone()));

    let enqueue_model = model.clone();
    let enqueue_daemon = daemon.clone();
    sim.spawner().spawn_with_cx(move |_cx| async move {
        enqueue_model.record_enqueue(REAL_WORKER_JOB_ID);
        enqueue_daemon
            .enqueue_job(
                REAL_WORKER_JOB_ID,
                "engineer",
                "acme/service",
                Artifact {
                    item: json!(495),
                    kind: "issue".to_string(),
                },
                json!({"prompt": "prove real worker shell under lab", "issue": 495}),
            )
            .await;
    });

    sim.run_until(
        || {
            let state = model.snapshot();
            let trace = probe.snapshot();
            state.applies.get(REAL_WORKER_JOB_ID).copied() == Some(1)
                && trace.registers > 0
                && trace.polls > 0
                && trace.assigned(REAL_WORKER_JOB_ID)
                && trace.submitted_success(REAL_WORKER_JOB_ID)
                && trace.accepted_release(REAL_WORKER_JOB_ID)
        },
        crate::scenarios::MAX_STEPS,
    );

    RealWorkerOutcome {
        model: model.snapshot(),
        trace: probe.snapshot(),
    }
}
