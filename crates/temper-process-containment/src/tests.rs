#![cfg(any(unix, windows))]

use std::io;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::*;

#[derive(Debug)]
struct FakeKernelState {
    members: Mutex<Vec<ProcessIdentity>>,
    inspection_blocked: AtomicBool,
    term_fails: AtomicBool,
    kill_failures_remaining: AtomicUsize,
    reuse_pid_on_term: AtomicBool,
    term_calls: AtomicUsize,
    kill_calls: AtomicUsize,
    emergency_term_calls: AtomicUsize,
    emergency_kill_calls: AtomicUsize,
    verify_blocked: AtomicBool,
    verify_stalled: AtomicBool,
}

impl FakeKernelState {
    fn new(members: Vec<ProcessIdentity>) -> Self {
        Self {
            members: Mutex::new(members),
            inspection_blocked: AtomicBool::new(false),
            term_fails: AtomicBool::new(false),
            kill_failures_remaining: AtomicUsize::new(0),
            reuse_pid_on_term: AtomicBool::new(false),
            term_calls: AtomicUsize::new(0),
            kill_calls: AtomicUsize::new(0),
            emergency_term_calls: AtomicUsize::new(0),
            emergency_kill_calls: AtomicUsize::new(0),
            verify_blocked: AtomicBool::new(false),
            verify_stalled: AtomicBool::new(false),
        }
    }
}

struct FakeKernel {
    kind: ContainmentBackendKind,
    root: ContainmentRootIdentity,
    state: Arc<FakeKernelState>,
    inspections: u64,
}

impl ContainmentKernel for FakeKernel {
    fn backend_kind(&self) -> ContainmentBackendKind {
        self.kind
    }

    fn root_identity(&self) -> ContainmentRootIdentity {
        self.root.clone()
    }

    fn discover_members(&mut self) -> io::Result<MemberDiscovery> {
        if self.state.inspection_blocked.load(Ordering::Acquire) {
            return Err(io::Error::other("injected membership inspection failure"));
        }
        self.inspections = self.inspections.saturating_add(1);
        let members = self
            .state
            .members
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Ok(MemberDiscovery::new(members, 0))
    }

    fn signal_members(&mut self, signal: ContainmentSignal) -> io::Result<SignalBatch> {
        if self.state.inspection_blocked.load(Ordering::Acquire) {
            return Err(io::Error::other("injected signal inspection failure"));
        }
        let mut members = self
            .state
            .members
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let targets = members.clone();
        let attempts = match signal {
            ContainmentSignal::Term => {
                self.state.term_calls.fetch_add(1, Ordering::AcqRel);
                if self.state.reuse_pid_on_term.swap(false, Ordering::AcqRel) {
                    let attempts = targets
                        .iter()
                        .cloned()
                        .map(|process| SignalAttempt::pid_reused(process, signal))
                        .collect();
                    *members = targets
                        .into_iter()
                        .map(|process| {
                            ProcessIdentity::new(
                                process.pid(),
                                process.ppid(),
                                process.process_group_id(),
                                process.session_id(),
                                process.start_time_identity().saturating_add(1),
                                "/fake/reused-pid",
                            )
                        })
                        .collect();
                    attempts
                } else if self.state.term_fails.load(Ordering::Acquire) {
                    targets
                        .into_iter()
                        .map(|process| SignalAttempt::failed(process, signal, "injected EPERM"))
                        .collect()
                } else {
                    members.clear();
                    targets
                        .into_iter()
                        .map(|process| SignalAttempt::succeeded(process, signal))
                        .collect()
                }
            }
            ContainmentSignal::Kill => {
                self.state.kill_calls.fetch_add(1, Ordering::AcqRel);
                let should_fail = self
                    .state
                    .kill_failures_remaining
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok();
                if should_fail {
                    targets
                        .into_iter()
                        .map(|process| SignalAttempt::failed(process, signal, "injected EACCES"))
                        .collect()
                } else {
                    members.clear();
                    targets
                        .into_iter()
                        .map(|process| SignalAttempt::succeeded(process, signal))
                        .collect()
                }
            }
        };
        Ok(SignalBatch::new(attempts, 0))
    }

    fn reap_direct_child(&mut self, child: &mut Child) -> io::Result<DirectChildReap> {
        let pid = child.id();
        match child.try_wait()? {
            Some(status) => Ok(DirectChildReap::Reaped {
                pid,
                exit_code: status.code(),
            }),
            None => Ok(DirectChildReap::Pending { pid }),
        }
    }

    fn verify_recursive_empty(&mut self) -> io::Result<RecursiveEmptyProof> {
        while self.state.verify_stalled.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
        if self.state.inspection_blocked.load(Ordering::Acquire)
            || self.state.verify_blocked.load(Ordering::Acquire)
        {
            return Err(io::Error::other("injected emptiness inspection failure"));
        }
        let members = self
            .state
            .members
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if members.is_empty() {
            Ok(RecursiveEmptyProof::proven(self.inspections))
        } else {
            Ok(RecursiveEmptyProof::not_empty(members, 0))
        }
    }

    fn wait(&mut self, _duration: Duration) {
        thread::sleep(Duration::from_millis(1));
    }
}

struct FakePrepared {
    kind: ContainmentBackendKind,
    root: ContainmentRootIdentity,
    state: Arc<FakeKernelState>,
    spawn_count: Arc<AtomicUsize>,
}

impl PreparedContainmentBackend for FakePrepared {
    fn kind(&self) -> ContainmentBackendKind {
        self.kind
    }

    fn root_identity(&self) -> ContainmentRootIdentity {
        self.root.clone()
    }

    fn spawn_precontained(
        self: Box<Self>,
        command: ContainmentCommand,
    ) -> io::Result<BackendSpawn> {
        self.spawn_count.fetch_add(1, Ordering::AcqRel);
        let child = command.into_std_command().spawn()?;
        let kernel = FakeKernel {
            kind: self.kind,
            root: self.root.clone(),
            state: Arc::clone(&self.state),
            inspections: 0,
        };
        let forced_state = Arc::clone(&self.state);
        let hard_kill_state = Arc::clone(&self.state);
        let emergency = EmergencyTerminationHandle::from_owners(
            "fake",
            move || {
                forced_state
                    .emergency_term_calls
                    .fetch_add(1, Ordering::AcqRel);
                if !forced_state.term_fails.load(Ordering::Acquire) {
                    forced_state
                        .members
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clear();
                }
                Ok(())
            },
            move || {
                hard_kill_state
                    .emergency_kill_calls
                    .fetch_add(1, Ordering::AcqRel);
                hard_kill_state
                    .members
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clear();
                Ok(())
            },
        )?;
        Ok(BackendSpawn::new(child, Box::new(kernel), emergency))
    }
}

struct FakeBackendFactory {
    kind: ContainmentBackendKind,
    state: Arc<FakeKernelState>,
    prepare_fails: AtomicBool,
    spawn_count: Arc<AtomicUsize>,
    requested_policies: Mutex<Vec<ContainmentBackendPolicy>>,
}

impl FakeBackendFactory {
    fn new(kind: ContainmentBackendKind, members: Vec<ProcessIdentity>) -> Self {
        Self {
            kind,
            state: Arc::new(FakeKernelState::new(members)),
            prepare_fails: AtomicBool::new(false),
            spawn_count: Arc::new(AtomicUsize::new(0)),
            requested_policies: Mutex::new(Vec::new()),
        }
    }
}

impl ContainmentBackendFactory for FakeBackendFactory {
    fn prepare_backend(
        &self,
        policy: ContainmentBackendPolicy,
        spec: &ContainmentSpec,
    ) -> io::Result<Box<dyn PreparedContainmentBackend>> {
        self.requested_policies
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(policy);
        if self.prepare_fails.load(Ordering::Acquire) {
            return Err(io::Error::other("injected prepare failure"));
        }
        let root = ContainmentRootIdentity::new(
            self.kind,
            format!("fake-root/{}", spec.identity.as_str()),
        );
        Ok(Box::new(FakePrepared {
            kind: self.kind,
            root,
            state: Arc::clone(&self.state),
            spawn_count: Arc::clone(&self.spawn_count),
        }))
    }
}

#[derive(Default)]
struct RecordingObserver {
    snapshots: Mutex<Vec<CleanupSnapshot>>,
    observations: Mutex<Vec<CleanupObservation>>,
    changed: Condvar,
}

impl RecordingObserver {
    fn wait_for_blocked(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut snapshots = self
            .snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if snapshots
                .iter()
                .any(|snapshot| matches!(snapshot, CleanupSnapshot::Blocked { .. }))
            {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, result) = self
                .changed
                .wait_timeout(snapshots, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            snapshots = next;
            if result.timed_out() {
                return snapshots
                    .iter()
                    .any(|snapshot| matches!(snapshot, CleanupSnapshot::Blocked { .. }));
            }
        }
    }
}

impl CleanupObserver for RecordingObserver {
    fn observe(&self, snapshot: &CleanupSnapshot) {
        self.snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(snapshot.clone());
        self.changed.notify_all();
    }

    fn observe_cleanup(&self, observation: &CleanupObservation) {
        self.observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(observation.clone());
        self.observe(observation.snapshot());
    }
}

fn process_identity(pid: u32) -> ProcessIdentity {
    ProcessIdentity::new(pid, 1, pid, pid, u64::from(pid) * 10, "/fake/temper-agent")
}

fn spec(name: &str) -> ContainmentSpec {
    ContainmentSpec::new(
        ContainmentIdentity::new(name).expect("valid test identity"),
        ContainmentScope::Tool,
    )
    .with_timing(Duration::from_millis(1), Duration::from_millis(1))
}

fn exited_command() -> ContainmentCommand {
    #[cfg(unix)]
    {
        let mut command = ContainmentCommand::new("sh");
        command.args(["-c", "exit 0"]);
        command
    }
    #[cfg(windows)]
    {
        let mut command = ContainmentCommand::new("cmd");
        command.args(["/C", "exit", "0"]);
        command
    }
}

fn factory_with_observer(
    fake: &Arc<FakeBackendFactory>,
    policy: ContainmentBackendPolicy,
    observer: Option<Arc<dyn CleanupObserver>>,
) -> ContainmentFactory {
    let backend: Arc<dyn ContainmentBackendFactory> = fake.clone();
    let factory = ContainmentFactory::new(policy, backend);
    match observer {
        Some(observer) => factory.with_observer(observer),
        None => factory,
    }
}

#[test]
fn prepare_failure_prevents_spawn() {
    let fake = Arc::new(FakeBackendFactory::new(
        ContainmentBackendKind::LinuxSupervisor,
        Vec::new(),
    ));
    fake.prepare_fails.store(true, Ordering::Release);
    let factory =
        factory_with_observer(&fake, ContainmentBackendPolicy::ForceLinuxSupervisor, None);

    let error = match factory.prepare(spec("prepare-failure")) {
        Ok(_) => panic!("prepare unexpectedly succeeded"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("prepare failure"));
    assert_eq!(fake.spawn_count.load(Ordering::Acquire), 0);
}

#[test]
fn cleanup_runs_exactly_once_and_first_trigger_wins() {
    let fake = Arc::new(FakeBackendFactory::new(
        ContainmentBackendKind::LinuxSupervisor,
        vec![process_identity(101)],
    ));
    let factory =
        factory_with_observer(&fake, ContainmentBackendPolicy::ForceLinuxSupervisor, None);
    let process = factory
        .prepare(spec("exactly-once"))
        .expect("prepare fake containment")
        .spawn(exited_command())
        .expect("spawn fake containment");

    let first = process.cleanup(CleanupTrigger::Cancellation);
    let second = process.cleanup(CleanupTrigger::Shutdown);

    assert_eq!(first, second);
    assert_eq!(first.trigger(), CleanupTrigger::Cancellation);
    assert_eq!(fake.state.term_calls.load(Ordering::Acquire), 1);
    assert_eq!(fake.state.kill_calls.load(Ordering::Acquire), 0);
    drop(process);
    assert_eq!(fake.state.term_calls.load(Ordering::Acquire), 1);
}

mod emergency;

#[test]
fn reports_bound_survivors_attempts_and_diagnostics() {
    let member_count = MAX_SURVIVOR_IDENTITIES + MAX_SIGNAL_ATTEMPTS + 17;
    let members = (1..=member_count)
        .map(|pid| process_identity(u32::try_from(pid).expect("test pid fits u32")))
        .collect();
    let fake = Arc::new(FakeBackendFactory::new(
        ContainmentBackendKind::LinuxSupervisor,
        members,
    ));
    fake.state.term_fails.store(true, Ordering::Release);
    fake.state
        .kill_failures_remaining
        .store(1, Ordering::Release);
    let factory =
        factory_with_observer(&fake, ContainmentBackendPolicy::ForceLinuxSupervisor, None);
    let process = factory
        .prepare(spec("bounded-report"))
        .expect("prepare fake containment")
        .spawn(exited_command())
        .expect("spawn fake containment");

    let report = process.cleanup(CleanupTrigger::Timeout);

    assert_eq!(report.disposition(), CleanupDisposition::Killed);
    assert_eq!(report.term_attempts().len(), MAX_SIGNAL_ATTEMPTS);
    assert!(report.omitted_term_attempts() > 0);
    assert_eq!(report.kill_attempts().len(), MAX_SIGNAL_ATTEMPTS);
    assert!(report.omitted_kill_attempts() > 0);
    assert_eq!(report.observed_survivors().len(), MAX_SURVIVOR_IDENTITIES);
    assert!(report.omitted_survivors() > 0);
    assert!(report.blocked_diagnostics().len() <= MAX_CLEANUP_DIAGNOSTICS);
    assert!(report.direct_child_reap().is_terminal());
    assert!(matches!(
        report.recursive_empty(),
        RecursiveEmptyProof::Proven { .. }
    ));
    assert!(
        report
            .term_attempts()
            .iter()
            .all(|attempt| matches!(attempt.outcome(), SignalAttemptOutcome::Failed(_)))
    );
    assert!(
        report
            .kill_attempts()
            .iter()
            .all(|attempt| matches!(attempt.outcome(), SignalAttemptOutcome::Failed(_)))
    );
}

#[test]
fn pid_reuse_is_structured_and_never_signals_the_reused_identity_as_the_old_process() {
    let fake = Arc::new(FakeBackendFactory::new(
        ContainmentBackendKind::LinuxSupervisor,
        vec![process_identity(301)],
    ));
    fake.state.reuse_pid_on_term.store(true, Ordering::Release);
    let factory =
        factory_with_observer(&fake, ContainmentBackendPolicy::ForceLinuxSupervisor, None);
    let process = factory
        .prepare(spec("pid-reuse"))
        .expect("prepare fake containment")
        .spawn(exited_command())
        .expect("spawn fake containment");

    let report = process.cleanup(CleanupTrigger::NormalRootExit);

    assert!(matches!(
        report.term_attempts()[0].outcome(),
        SignalAttemptOutcome::PidReused
    ));
    assert_eq!(
        report.kill_attempts()[0].process().start_time_identity(),
        process_identity(301).start_time_identity() + 1
    );
    assert_eq!(report.disposition(), CleanupDisposition::Killed);
}

#[test]
fn backend_policy_is_injected_per_factory_instance() {
    let cgroup = Arc::new(FakeBackendFactory::new(
        ContainmentBackendKind::LinuxCgroupV2,
        Vec::new(),
    ));
    let supervisor = Arc::new(FakeBackendFactory::new(
        ContainmentBackendKind::LinuxSupervisor,
        Vec::new(),
    ));
    let cgroup_factory =
        factory_with_observer(&cgroup, ContainmentBackendPolicy::RequireCgroupV2, None);
    let supervisor_factory = factory_with_observer(
        &supervisor,
        ContainmentBackendPolicy::ForceLinuxSupervisor,
        None,
    );

    let cgroup_prepared = cgroup_factory
        .prepare(spec("cgroup"))
        .expect("select fake cgroup");
    let supervisor_prepared = supervisor_factory
        .prepare(spec("supervisor"))
        .expect("select fake supervisor");

    assert_eq!(
        cgroup_prepared.backend_kind(),
        ContainmentBackendKind::LinuxCgroupV2
    );
    assert_eq!(
        supervisor_prepared.backend_kind(),
        ContainmentBackendKind::LinuxSupervisor
    );
    assert_eq!(
        cgroup
            .requested_policies
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        &[ContainmentBackendPolicy::RequireCgroupV2]
    );
    assert_eq!(
        supervisor
            .requested_policies
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        &[ContainmentBackendPolicy::ForceLinuxSupervisor]
    );

    let mismatched =
        factory_with_observer(&supervisor, ContainmentBackendPolicy::RequireCgroupV2, None);
    assert!(mismatched.prepare(spec("mismatch")).is_err());
}

#[test]
fn command_owns_spawn_inputs_and_legacy_adapter_is_marked_incomplete() {
    let mut command = ContainmentCommand::new("program");
    command
        .args(["one", "two"])
        .env("SET", "value")
        .env_remove("REMOVE")
        .env_clear()
        .current_dir("workspace")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    assert_eq!(command.program(), "program");
    assert_eq!(command.arguments(), &["one", "two"]);
    assert_eq!(command.environment_changes().len(), 2);
    assert!(command.clears_environment());
    assert_eq!(command.cwd(), Some(std::path::Path::new("workspace")));

    #[cfg(unix)]
    {
        let mut legacy_command = std::process::Command::new("sh");
        legacy_command.args(["-c", "exit 0"]);
        configure_command(&mut legacy_command);
        let mut child = legacy_command.spawn().expect("spawn legacy child");
        let legacy = ProcessContainment::attach(&child).expect("attach legacy adapter");
        assert!(!legacy.is_descendant_complete());
        child.wait().expect("reap legacy child");
        legacy
            .hard_kill(&mut child)
            .expect("empty legacy group is harmless");
    }
}
