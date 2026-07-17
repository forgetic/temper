use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use skein::cx::Cx;
use temper_process_containment::{
    BackendSpawn, ContainmentBackendFactory, ContainmentBackendKind, ContainmentBackendPolicy,
    ContainmentCommand, ContainmentFactory, ContainmentKernel, ContainmentRootIdentity,
    ContainmentSignal, ContainmentSpec, DirectChildReap, MemberDiscovery,
    PreparedContainmentBackend, RecursiveEmptyProof, SignalBatch,
};
use temper_protocol_worker::{
    Artifact, Assign, FailureClass, WORKER_PROTOCOL_VERSION, WorkerAuth, WorkerProtocolMessage,
};
use temper_testing::descendant_fixture::{current_exact_identity, read_recorded_identities};
use temper_worker::{
    ActiveJobJoinState, AgentRunRequest, AgentRunner, ExecutorSelection, JobCancellation,
    JobExecutionContext, JobExecutor, JobOutcome, OutOfProcessRunner, Transport,
    WorkerAgentTraceConfig, WorkerConfig, WorkerLivenessLimits, shutdown_worker_after_signal,
    start_worker_with_transport,
};

use super::{BackendMode, FixtureCase};

const POLL: Duration = Duration::from_millis(2);
const WAIT: Duration = Duration::from_secs(5);

pub(super) fn watchdog_capacity_one(mode: BackendMode, fixture: &Path) -> io::Result<()> {
    run_worker_case(mode, fixture, WorkerCase::Watchdog)
}

pub(super) fn inspection_failure_retains_capacity(
    mode: BackendMode,
    fixture: &Path,
) -> io::Result<()> {
    run_worker_case(mode, fixture, WorkerCase::InspectionFailure)
}

pub(super) fn signal_shutdown(
    mode: BackendMode,
    fixture: &Path,
    standalone: bool,
) -> io::Result<()> {
    run_worker_case(mode, fixture, WorkerCase::SignalShutdown { standalone })
}

#[derive(Clone, Copy)]
enum WorkerCase {
    Watchdog,
    InspectionFailure,
    SignalShutdown { standalone: bool },
}

impl WorkerCase {
    fn label(self) -> &'static str {
        match self {
            Self::Watchdog => "watchdog",
            Self::InspectionFailure => "inspection",
            Self::SignalShutdown { standalone: false } => "split-shutdown",
            Self::SignalShutdown { standalone: true } => "standalone-shutdown",
        }
    }

    fn uses_watchdog(self) -> bool {
        !matches!(self, Self::SignalShutdown { .. })
    }
}

fn run_worker_case(mode: BackendMode, fixture: &Path, kind: WorkerCase) -> io::Result<()> {
    let mut case = FixtureCase::start(
        fixture,
        &format!("worker-{}-{}", kind.label(), mode.label()),
    )?;
    std::fs::create_dir(case.temporary.path().join("demo"))?;
    let inspection_blocked =
        matches!(kind, WorkerCase::InspectionFailure).then(|| Arc::new(AtomicBool::new(true)));
    let cleanup_complete = Arc::new(AtomicBool::new(false));
    let runner = fixture_runner(
        mode,
        case.agent_command(0, true),
        inspection_blocked.clone(),
    );
    let executor = Arc::new(FixtureExecutor::new(
        runner,
        case.temporary.path().to_path_buf(),
        Arc::clone(&cleanup_complete),
    ));
    let transport = Arc::new(CapacityOneTransport::new(Arc::clone(&cleanup_complete)));
    let result_root = case.temporary.path().join("results");
    let ready = case.ready.clone();

    temper_worker_io::block_on_with(move |_cx, handle| async move {
        let worker = start_worker_with_transport(
            handle,
            worker_config(result_root, kind.uses_watchdog()),
            Arc::clone(&executor),
            Arc::clone(&transport),
        );
        let registry = worker.task_registry();
        wait_until("fixture ready", || ready.exists()).await?;

        match kind {
            WorkerCase::Watchdog => {
                wait_until("watchdog cancellation", || executor.cancel_requested()).await?;
                assert_capacity_retained(&case, &executor, &transport, &registry)?;
                wait_until("watchdog cleanup", || {
                    cleanup_complete.load(Ordering::Acquire)
                })
                .await?;
                case.finish(3)?;
                wait_for_second_dispatch(&executor, &transport).await?;
                worker.shutdown().await;
            }
            WorkerCase::InspectionFailure => {
                wait_until("cleanup-pending registry state", || {
                    registry
                        .active_jobs()
                        .iter()
                        .any(|task| task.join_state() == ActiveJobJoinState::CleanupPending)
                })
                .await?;
                assert_capacity_retained(&case, &executor, &transport, &registry)?;
                temper_worker_io::sleep_for(Duration::from_millis(20)).await;
                assert_capacity_retained(&case, &executor, &transport, &registry)?;
                inspection_blocked
                    .as_ref()
                    .expect("inspection case has a fault switch")
                    .store(false, Ordering::Release);
                wait_until("inspection recovery", || {
                    cleanup_complete.load(Ordering::Acquire)
                })
                .await?;
                case.finish(3)?;
                wait_for_second_dispatch(&executor, &transport).await?;
                worker.shutdown().await;
            }
            WorkerCase::SignalShutdown { standalone } => {
                let before_join = Arc::new(AtomicBool::new(false));
                let before_join_task = Arc::clone(&before_join);
                let before_registry = registry.clone();
                let before_identities = case.identities.clone();
                let before = async move {
                    assert_eq!(before_registry.active_jobs().len(), 1);
                    assert!(recorded_fixture_is_alive(&before_identities).unwrap());
                    before_join_task.store(true, Ordering::Release);
                };
                let assignment_released = Arc::new(AtomicBool::new(false));
                if standalone {
                    let released = Arc::clone(&assignment_released);
                    let release_registry = registry.clone();
                    let release_identities = case.identities.clone();
                    let release = async move {
                        assert!(release_registry.is_empty());
                        assert!(!recorded_fixture_is_alive(&release_identities).unwrap());
                        released.store(true, Ordering::Release);
                    };
                    shutdown_worker_after_signal(std::future::ready(()), before, worker, release)
                        .await;
                } else {
                    shutdown_worker_after_signal(
                        std::future::ready(()),
                        before,
                        worker,
                        std::future::ready(()),
                    )
                    .await;
                }
                if !before_join.load(Ordering::Acquire) {
                    return Err(io::Error::other("signal path skipped its worker join"));
                }
                if standalone != assignment_released.load(Ordering::Acquire) {
                    return Err(io::Error::other(
                        "standalone assignment-release ordering was not enforced",
                    ));
                }
                if !registry.is_empty() || !cleanup_complete.load(Ordering::Acquire) {
                    return Err(io::Error::other(
                        "signal shutdown returned before active fixture cleanup joined",
                    ));
                }
                case.finish(3)?;
                if transport.polls() != 1 || executor.dispatches() != 1 || transport.results() != 0
                {
                    return Err(io::Error::other(format!(
                        "shutdown accepted work or published a result: polls={} dispatches={} results={}",
                        transport.polls(),
                        executor.dispatches(),
                        transport.results(),
                    )));
                }
            }
        }

        assert_cleanup_backend(
            mode,
            &executor,
            matches!(kind, WorkerCase::InspectionFailure),
        )
    })
}

fn assert_capacity_retained(
    case: &FixtureCase,
    executor: &FixtureExecutor,
    transport: &CapacityOneTransport,
    registry: &temper_worker::WorkerTaskRegistry,
) -> io::Result<()> {
    if transport.polls() != 1
        || transport.premature_poll()
        || executor.dispatches() != 1
        || registry.active_jobs().len() != 1
        || executor.cleanup_complete.load(Ordering::Acquire)
        || !recorded_fixture_is_alive(&case.identities)?
    {
        return Err(io::Error::other(format!(
            "capacity escaped before cleanup: polls={} premature={} dispatches={} active={} cleanup={}",
            transport.polls(),
            transport.premature_poll(),
            executor.dispatches(),
            registry.active_jobs().len(),
            executor.cleanup_complete.load(Ordering::Acquire),
        )));
    }
    Ok(())
}

async fn wait_for_second_dispatch(
    executor: &FixtureExecutor,
    transport: &CapacityOneTransport,
) -> io::Result<()> {
    wait_until("post-cleanup second dispatch", || {
        executor.dispatches() == 2
    })
    .await?;
    if transport.polls() < 2 || transport.premature_poll() {
        return Err(io::Error::other(format!(
            "second dispatch ordering was invalid: polls={} premature={}",
            transport.polls(),
            transport.premature_poll(),
        )));
    }
    Ok(())
}

fn assert_cleanup_backend(
    mode: BackendMode,
    executor: &FixtureExecutor,
    recovered_inspection: bool,
) -> io::Result<()> {
    let cleanup = executor
        .first_cancellation()
        .and_then(|cancellation| cancellation.cleanup())
        .ok_or_else(|| io::Error::other("worker fixture omitted its cleanup proof"))?;
    let expected = match mode {
        BackendMode::ForcedSupervisor => ContainmentBackendKind::LinuxSupervisor,
        BackendMode::AutoCgroup => ContainmentBackendKind::LinuxCgroupV2,
    };
    if cleanup.containment.backend() != expected
        || !cleanup.proves_quiescence()
        || (recovered_inspection && cleanup.containment.blocked_diagnostics().is_empty())
    {
        return Err(io::Error::other(format!(
            "worker fixture cleanup evidence was incomplete: {cleanup:?}"
        )));
    }
    Ok(())
}

struct FixtureExecutor {
    runner: OutOfProcessRunner,
    cwd: PathBuf,
    context: temper_protocol_agent::WorkspaceContext,
    dispatches: AtomicUsize,
    cleanup_complete: Arc<AtomicBool>,
    first_cancellation: Mutex<Option<JobCancellation>>,
}

impl FixtureExecutor {
    fn new(runner: OutOfProcessRunner, cwd: PathBuf, cleanup_complete: Arc<AtomicBool>) -> Self {
        Self {
            runner,
            cwd,
            context: super::workspace_context(),
            dispatches: AtomicUsize::new(0),
            cleanup_complete,
            first_cancellation: Mutex::new(None),
        }
    }

    fn dispatches(&self) -> usize {
        self.dispatches.load(Ordering::Acquire)
    }

    fn first_cancellation(&self) -> Option<JobCancellation> {
        self.first_cancellation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn cancel_requested(&self) -> bool {
        self.first_cancellation()
            .is_some_and(|cancellation| cancellation.requested().is_some())
    }
}

impl JobExecutor for FixtureExecutor {
    fn execute(
        &self,
        assign: Assign,
        execution: JobExecutionContext,
    ) -> impl Future<Output = JobOutcome> + Send {
        let dispatch = self.dispatches.fetch_add(1, Ordering::AcqRel) + 1;
        let runner = self.runner.clone();
        let cwd = self.cwd.clone();
        let context = self.context.clone();
        let cleanup_complete = Arc::clone(&self.cleanup_complete);
        if dispatch == 1 {
            *self
                .first_cancellation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(execution.cancellation.clone());
        }
        async move {
            if dispatch > 1 {
                return JobOutcome::Failure {
                    class: FailureClass::Permanent,
                    message: "post-cleanup second dispatch".to_string(),
                };
            }
            let request = AgentRunRequest::new_controlled(
                &assign.job_id,
                execution.attempt.id,
                &context,
                &cwd,
                execution.fence,
                execution.cancellation,
                execution.progress,
            );
            let result = runner.run_request(request).await;
            cleanup_complete.store(true, Ordering::Release);
            JobOutcome::Failure {
                class: FailureClass::Canceled,
                message: format!("fixture agent completed after cancellation: {result:?}"),
            }
        }
    }
}

struct CapacityOneTransport {
    polls: AtomicUsize,
    results: AtomicUsize,
    premature_poll: AtomicBool,
    cleanup_complete: Arc<AtomicBool>,
}

impl CapacityOneTransport {
    fn new(cleanup_complete: Arc<AtomicBool>) -> Self {
        Self {
            polls: AtomicUsize::new(0),
            results: AtomicUsize::new(0),
            premature_poll: AtomicBool::new(false),
            cleanup_complete,
        }
    }

    fn polls(&self) -> usize {
        self.polls.load(Ordering::Acquire)
    }

    fn results(&self) -> usize {
        self.results.load(Ordering::Acquire)
    }

    fn premature_poll(&self) -> bool {
        self.premature_poll.load(Ordering::Acquire)
    }
}

impl Transport for CapacityOneTransport {
    fn send(
        &self,
        _cx: Cx,
        message: WorkerProtocolMessage,
        _auth: Option<WorkerAuth>,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        let reply = match message {
            WorkerProtocolMessage::Register(_) => Ok(None),
            WorkerProtocolMessage::Poll(_) => {
                let poll = self.polls.fetch_add(1, Ordering::AcqRel) + 1;
                if poll > 1 && !self.cleanup_complete.load(Ordering::Acquire) {
                    self.premature_poll.store(true, Ordering::Release);
                }
                Ok((poll <= 2).then(|| WorkerProtocolMessage::Assign(assignment(poll))))
            }
            WorkerProtocolMessage::Result(_) => {
                self.results.fetch_add(1, Ordering::AcqRel);
                Ok(None)
            }
            _ => Ok(None),
        };
        std::future::ready(reply)
    }
}

fn assignment(number: usize) -> Assign {
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        trace_context: None,
        job_id: format!("fixture-job-{number}"),
        attempt_id: Some(format!("fixture-attempt-{number}")),
        role: "engineer".to_string(),
        repo: "ai/fixture".to_string(),
        artifact: Artifact {
            item: serde_json::json!(468),
            kind: "issue".to_string(),
        },
        job_payload: serde_json::json!({}),
    }
}

fn worker_config(result_root: PathBuf, watchdog: bool) -> WorkerConfig {
    WorkerConfig {
        daemon_url: String::new(),
        worker_id: "descendant-acceptance".to_string(),
        worker_pool: None,
        worker_auth: None,
        capabilities: Vec::new(),
        role_identities: BTreeMap::new(),
        max_concurrent_jobs: 1,
        poll_wait: POLL,
        heartbeat_interval: Duration::from_secs(60),
        liveness_limits: WorkerLivenessLimits {
            max_no_progress: if watchdog {
                Duration::from_millis(40)
            } else {
                Duration::from_secs(60)
            },
            max_run: None,
            graceful_cancellation_grace: Duration::from_millis(20),
            forced_termination_grace: Duration::from_millis(20),
        },
        result_root,
        agent_traces: WorkerAgentTraceConfig::default(),
        executor: ExecutorSelection::Stub,
    }
}

fn fixture_runner(
    mode: BackendMode,
    command: Vec<String>,
    inspection_blocked: Option<Arc<AtomicBool>>,
) -> OutOfProcessRunner {
    let limits = WorkerLivenessLimits {
        graceful_cancellation_grace: Duration::from_millis(20),
        forced_termination_grace: Duration::from_millis(20),
        ..WorkerLivenessLimits::default()
    };
    OutOfProcessRunner::new(command)
        .with_liveness_limits(limits)
        .with_containment_factory(move |job, attempt| {
            containment_factory(mode, job, attempt, inspection_blocked.clone())
        })
}

fn containment_factory(
    mode: BackendMode,
    job: &str,
    attempt: &str,
    inspection_blocked: Option<Arc<AtomicBool>>,
) -> io::Result<ContainmentFactory> {
    let (policy, backend) = super::backend_factory(mode, job, attempt)?;
    let backend = match inspection_blocked {
        Some(blocked) => Arc::new(InspectionFaultFactory { backend, blocked })
            as Arc<dyn ContainmentBackendFactory>,
        None => backend,
    };
    Ok(ContainmentFactory::new(policy, backend))
}

struct InspectionFaultFactory {
    backend: Arc<dyn ContainmentBackendFactory>,
    blocked: Arc<AtomicBool>,
}

impl ContainmentBackendFactory for InspectionFaultFactory {
    fn prepare_backend(
        &self,
        policy: ContainmentBackendPolicy,
        spec: &ContainmentSpec,
    ) -> io::Result<Box<dyn PreparedContainmentBackend>> {
        let backend = self.backend.prepare_backend(policy, spec)?;
        Ok(Box::new(InspectionFaultPrepared {
            backend,
            blocked: Arc::clone(&self.blocked),
        }))
    }

    fn capability_diagnostic(
        &self,
        selected: ContainmentBackendKind,
    ) -> Option<temper_process_containment::ContainmentCapabilityDiagnostic> {
        self.backend.capability_diagnostic(selected)
    }
}

struct InspectionFaultPrepared {
    backend: Box<dyn PreparedContainmentBackend>,
    blocked: Arc<AtomicBool>,
}

impl PreparedContainmentBackend for InspectionFaultPrepared {
    fn kind(&self) -> ContainmentBackendKind {
        self.backend.kind()
    }

    fn root_identity(&self) -> ContainmentRootIdentity {
        self.backend.root_identity()
    }

    fn spawn_precontained(
        self: Box<Self>,
        command: ContainmentCommand,
    ) -> io::Result<BackendSpawn> {
        let Self { backend, blocked } = *self;
        backend.spawn_precontained(command).map(|spawn| {
            spawn.map_kernel(|kernel| Box::new(InspectionFaultKernel { kernel, blocked }))
        })
    }
}

struct InspectionFaultKernel {
    kernel: Box<dyn ContainmentKernel>,
    blocked: Arc<AtomicBool>,
}

impl InspectionFaultKernel {
    fn inspect(&self) -> io::Result<()> {
        if self.blocked.load(Ordering::Acquire) {
            Err(io::Error::other(
                "injected owner membership inspection failure",
            ))
        } else {
            Ok(())
        }
    }
}

impl ContainmentKernel for InspectionFaultKernel {
    fn backend_kind(&self) -> ContainmentBackendKind {
        self.kernel.backend_kind()
    }

    fn root_identity(&self) -> ContainmentRootIdentity {
        self.kernel.root_identity()
    }

    fn discover_members(&mut self) -> io::Result<MemberDiscovery> {
        self.inspect()?;
        self.kernel.discover_members()
    }

    fn signal_members(&mut self, signal: ContainmentSignal) -> io::Result<SignalBatch> {
        self.inspect()?;
        self.kernel.signal_members(signal)
    }

    fn take_backend_signal_batch(&mut self, signal: ContainmentSignal) -> Option<SignalBatch> {
        self.kernel.take_backend_signal_batch(signal)
    }

    fn reap_direct_child(
        &mut self,
        child: &mut std::process::Child,
    ) -> io::Result<DirectChildReap> {
        self.kernel.reap_direct_child(child)
    }

    fn verify_recursive_empty(&mut self) -> io::Result<RecursiveEmptyProof> {
        self.inspect()?;
        self.kernel.verify_recursive_empty()
    }

    fn wait(&mut self, duration: Duration) {
        self.kernel.wait(duration);
    }
}

async fn wait_until(label: &str, condition: impl Fn() -> bool) -> io::Result<()> {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out waiting for {label}"),
            ));
        }
        temper_worker_io::sleep_for(POLL).await;
    }
    Ok(())
}

fn recorded_fixture_is_alive(path: &Path) -> io::Result<bool> {
    for identity in read_recorded_identities(path)? {
        if current_exact_identity(&identity)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}
