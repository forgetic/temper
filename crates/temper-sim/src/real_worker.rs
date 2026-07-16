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
    Artifact, FailureClass, JobHeartbeatPhase, JobTimeoutReason, ReleaseDisposition, ResultStatus,
    WorkerAuth, WorkerProtocolMessage,
};
use temper_worker::{
    CapabilitySpec, ExecutorSelection, JobExecutor, ResultOutbox, StubExecutor, Transport,
    WorkerConfig, WorkerLivenessLimits, run_worker_with_transport,
};

use crate::model::SimModel;
use crate::{InProcessTransport, Sim};

mod controllable;
pub use controllable::{ControllableExecutor, ControllableExecutorState};

/// Default real-worker scenario job id used by [`run_success_stub_worker_once`].
pub const REAL_WORKER_JOB_ID: &str = "acme/service/issue-real/engineer/code_ready";
/// Job whose Forge/context operation stalls in [`run_hung_forge_watchdog_once`].
pub const HUNG_FORGE_JOB_ID: &str = "acme/service/issue-hung/engineer/code_ready";
/// Unrelated job that must reuse capacity without a worker restart.
pub const FOLLOW_UP_JOB_ID: &str = "acme/service/issue-follow-up/engineer/code_ready";

/// Runtime/config profile for a real worker inside simulation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealWorkerProfile {
    pub worker_id: String,
    pub role: String,
    pub repo: String,
    pub max_concurrent_jobs: u32,
    pub poll_wait: Duration,
    pub heartbeat_interval: Duration,
    pub liveness_limits: WorkerLivenessLimits,
    /// Hermetic durable result root for this simulated worker process.
    pub result_root: std::path::PathBuf,
}

impl RealWorkerProfile {
    pub fn new(worker_id: impl Into<String>, role: &str, repo: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_RESULT_ROOT: AtomicU64 = AtomicU64::new(1);

        let worker_id = worker_id.into();
        let root_sequence = NEXT_RESULT_ROOT.fetch_add(1, Ordering::Relaxed);
        let result_root = std::env::temp_dir().join(format!(
            "temper-sim-worker-results-{}-{root_sequence}",
            std::process::id()
        ));
        // A prior abruptly terminated test process may have left this
        // process-scoped path behind after PID reuse.
        let _ = std::fs::remove_dir_all(&result_root);
        Self {
            result_root,
            worker_id,
            role: role.to_string(),
            repo: repo.to_string(),
            max_concurrent_jobs: 1,
            poll_wait: Duration::from_millis(50),
            heartbeat_interval: Duration::from_millis(50),
            liveness_limits: WorkerLivenessLimits::default(),
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
            liveness_limits: self.liveness_limits,
            result_root: self.result_root.clone(),
            agent_traces: Default::default(),
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
    pub result_failure_classes: Vec<(String, Option<FailureClass>)>,
    /// Result messages observed only after the exact payload was found in the
    /// restart-readable outbox.
    pub durable_result_sends: Vec<String>,
    pub releases: Vec<(String, ReleaseDisposition)>,
    pub liveness: Vec<(String, JobHeartbeatPhase, Option<JobTimeoutReason>)>,
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

    pub fn submitted_transient(&self, job_id: &str) -> bool {
        self.result_failure_classes
            .iter()
            .any(|(seen, class)| seen == job_id && *class == Some(FailureClass::Transient))
    }

    pub fn result_count(&self, job_id: &str) -> usize {
        self.results
            .iter()
            .filter(|(seen, _)| seen == job_id)
            .count()
    }

    pub fn released(&self, job_id: &str) -> bool {
        self.releases.iter().any(|(seen, _)| seen == job_id)
    }

    pub fn release_count(&self, job_id: &str) -> usize {
        self.releases
            .iter()
            .filter(|(seen, _)| seen == job_id)
            .count()
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
        result_root: &std::path::Path,
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
            WorkerProtocolMessage::Heartbeat(heartbeat) => {
                let mut trace = self.trace();
                trace.heartbeats += 1;
                trace
                    .liveness
                    .extend(heartbeat.jobs.iter().filter_map(|job| {
                        job.liveness.as_ref().map(|liveness| {
                            (
                                job.job_id.clone(),
                                liveness.phase,
                                liveness.timeout.as_ref().map(|timeout| timeout.reason),
                            )
                        })
                    }));
                None
            }
            WorkerProtocolMessage::Result(result) => {
                let durable = ResultOutbox::new(result_root)
                    .load()
                    .is_ok_and(|entries| entries.iter().any(|entry| entry.result == *result));
                let mut trace = self.trace();
                trace.results.push((result.job_id.clone(), result.status));
                trace.result_failure_classes.push((
                    result.job_id.clone(),
                    result.failure.as_ref().map(|failure| failure.class),
                ));
                if durable {
                    trace.durable_result_sends.push(result.job_id.clone());
                } else {
                    trace.transport_errors.push(format!(
                        "result {} reached transport before durable recording",
                        result.job_id
                    ));
                }
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
    result_root: std::path::PathBuf,
}

impl ObservedInProcessTransport {
    pub fn new(
        daemon: Daemon,
        probe: RealWorkerProbe,
        model: Option<SimModel>,
        result_root: std::path::PathBuf,
    ) -> Self {
        Self {
            inner: InProcessTransport::new(daemon),
            probe,
            model,
            result_root,
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
        let result_root = self.result_root.clone();
        async move {
            let reply = Transport::send(&inner, cx, message.clone(), auth).await;
            probe.record_exchange(&message, &reply, model.as_ref(), &result_root);
            reply
        }
    }
}

/// Spawn a production `WorkerMachine`/`WorkerShell` with a caller-controlled
/// executor on the lab runtime. The worker runs like production (forever);
/// tests drive the lab until their model/probe condition is met and then drop
/// the world.
pub fn spawn_real_worker<E>(
    sim: &Sim,
    daemon: &Daemon,
    profile: RealWorkerProfile,
    model: Option<SimModel>,
    executor: Arc<E>,
) -> RealWorkerProbe
where
    E: JobExecutor + Send + Sync + 'static,
{
    let probe = RealWorkerProbe::default();
    let transport = Arc::new(ObservedInProcessTransport::new(
        daemon.clone(),
        probe.clone(),
        model,
        profile.result_root.clone(),
    ));
    let config = profile.worker_config();
    let spawner = sim.spawner();
    let worker_spawner = spawner.clone();

    spawner.spawn_with_cx(move |_cx| async move {
        let _ = run_worker_with_transport(worker_spawner, config, executor, transport).await;
    });

    probe
}

/// Spawn the compatibility success-stub variant of [`spawn_real_worker`].
pub fn spawn_success_stub_worker(
    sim: &Sim,
    daemon: &Daemon,
    profile: RealWorkerProfile,
    model: Option<SimModel>,
) -> RealWorkerProbe {
    spawn_real_worker(
        sim,
        daemon,
        profile,
        model,
        Arc::new(StubExecutor::success()),
    )
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
    let result_root = profile.result_root.clone();
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

    let outcome = RealWorkerOutcome {
        model: model.snapshot(),
        trace: probe.snapshot(),
    };
    drop(sim);
    let _ = std::fs::remove_dir_all(result_root);
    outcome
}

/// Complete capacity-one watchdog scenario, including a late Forge completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LivenessRecoveryOutcome {
    pub model: crate::model::ModelState,
    pub trace_before_late_completion: RealWorkerTrace,
    pub trace_after_late_completion: RealWorkerTrace,
    pub executor_before_late_completion: ControllableExecutorState,
    pub executor_after_late_completion: ControllableExecutorState,
    pub late_progress_accepted: bool,
}

/// Run the production worker machine/shell under virtual time while its first
/// executor parks in a Forge/context future. Lease heartbeats continue, but do
/// not count as progress. The watchdog cancels and durably records the first
/// attempt, capacity is reused by an unrelated job, and the old future is then
/// resolved to exercise the attempt fence.
pub fn run_hung_forge_watchdog_once(seed: u64) -> LivenessRecoveryOutcome {
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
    .with_apply_grace(Duration::from_millis(20));

    let mut profile = RealWorkerProfile::new("watchdog-worker-0", "engineer", "acme/service");
    profile.poll_wait = Duration::from_millis(5);
    profile.heartbeat_interval = Duration::from_millis(5);
    profile.liveness_limits = WorkerLivenessLimits {
        max_no_progress: Duration::from_millis(30),
        max_run: None,
        graceful_cancellation_grace: Duration::from_millis(5),
        forced_termination_grace: Duration::from_millis(5),
    };
    let result_root = profile.result_root.clone();
    let executor = ControllableExecutor::with_hung_forge_job(HUNG_FORGE_JOB_ID);
    let probe = spawn_real_worker(
        &sim,
        &daemon,
        profile,
        Some(model.clone()),
        Arc::new(executor.clone()),
    );

    let first_model = model.clone();
    let first_daemon = daemon.clone();
    sim.spawner().spawn_with_cx(move |_cx| async move {
        first_model.record_enqueue(HUNG_FORGE_JOB_ID);
        first_daemon
            .enqueue_job(
                HUNG_FORGE_JOB_ID,
                "engineer",
                "acme/service",
                Artifact {
                    item: json!(496),
                    kind: "issue".to_string(),
                },
                json!({"prompt": "hang Forge context until cancelled", "issue": 496}),
            )
            .await;
    });

    // Observe several ordinary renewals while the typed Forge tool remains the
    // current operation. This prevents a false-positive test that merely races
    // assignment directly into timeout.
    sim.run_until(
        || {
            let trace = probe.snapshot();
            let state = executor.snapshot();
            trace.assigned(HUNG_FORGE_JOB_ID)
                && state.starts == [HUNG_FORGE_JOB_ID]
                && trace
                    .liveness
                    .iter()
                    .filter(|(job_id, phase, timeout)| {
                        job_id == HUNG_FORGE_JOB_ID
                            && *phase == JobHeartbeatPhase::Running
                            && timeout.is_none()
                    })
                    .count()
                    >= 3
        },
        crate::scenarios::MAX_STEPS,
    );

    let second_model = model.clone();
    let second_daemon = daemon.clone();
    sim.spawner().spawn_with_cx(move |_cx| async move {
        second_model.record_enqueue(FOLLOW_UP_JOB_ID);
        second_daemon
            .enqueue_job(
                FOLLOW_UP_JOB_ID,
                "engineer",
                "acme/service",
                Artifact {
                    item: json!(497),
                    kind: "issue".to_string(),
                },
                json!({"prompt": "run after watchdog capacity recovery", "issue": 497}),
            )
            .await;
    });

    sim.run_until(
        || {
            let state = model.snapshot();
            let trace = probe.snapshot();
            state.applies.get(HUNG_FORGE_JOB_ID).copied() == Some(1)
                && state.applies.get(FOLLOW_UP_JOB_ID).copied() == Some(1)
                && trace.submitted_transient(HUNG_FORGE_JOB_ID)
                && trace.submitted_success(FOLLOW_UP_JOB_ID)
                && trace.accepted_release(HUNG_FORGE_JOB_ID)
                && trace.accepted_release(FOLLOW_UP_JOB_ID)
                && executor.snapshot().starts
                    == [HUNG_FORGE_JOB_ID.to_string(), FOLLOW_UP_JOB_ID.to_string()]
        },
        crate::scenarios::MAX_STEPS,
    );

    let trace_before_late_completion = probe.snapshot();
    let executor_before_late_completion = executor.snapshot();
    let late_progress_accepted = executor.report_late_progress(HUNG_FORGE_JOB_ID);
    assert!(executor.resolve_forge_future(HUNG_FORGE_JOB_ID));

    // No job heartbeat exists after both releases, so use two subsequent
    // long-poll cycles as a deterministic scheduler barrier for the late wake.
    let polls_before_late_completion = trace_before_late_completion.polls;
    sim.run_until(
        || probe.snapshot().polls >= polls_before_late_completion.saturating_add(2),
        crate::scenarios::MAX_STEPS,
    );

    let outcome = LivenessRecoveryOutcome {
        model: model.snapshot(),
        trace_before_late_completion,
        trace_after_late_completion: probe.snapshot(),
        executor_before_late_completion,
        executor_after_late_completion: executor.snapshot(),
        late_progress_accepted,
    };
    drop(sim);
    let _ = std::fs::remove_dir_all(result_root);
    outcome
}
