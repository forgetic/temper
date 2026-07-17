use std::io;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, ExitStatus};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use crate::command::ContainmentCommand;
use crate::model::*;

mod evidence;
use evidence::collect_backend_signal_evidence;

/// Kernel/backend operations used by the implementation-neutral cleanup state
/// machine. Test fakes can implement this trait without mutating process-global
/// environment or backend selectors.
pub trait ContainmentKernel: Send {
    fn backend_kind(&self) -> ContainmentBackendKind;
    fn root_identity(&self) -> ContainmentRootIdentity;

    /// Inspect the complete containment and return a bounded diagnostic sample.
    /// An inability to inspect ownership must be returned as `Err`, never as an
    /// empty discovery.
    fn discover_members(&mut self) -> io::Result<MemberDiscovery>;

    /// Signal every member currently owned by this containment. The returned
    /// batch is diagnostic only and may be bounded; omitted members must still
    /// have been considered by the backend. PID/start-time mismatch must be
    /// represented as [`SignalAttemptOutcome::PidReused`].
    fn signal_members(&mut self, signal: ContainmentSignal) -> io::Result<SignalBatch>;

    /// Return a signal batch performed internally by a supervising backend
    /// before the shared owner-side state machine observed completion. Each
    /// batch may be taken at most once. Most kernels never signal internally.
    fn take_backend_signal_batch(&mut self, _signal: ContainmentSignal) -> Option<SignalBatch> {
        None
    }

    /// Non-blockingly reap the eligible direct child or report that it remains
    /// live. Inspection/reap uncertainty is an error and therefore fail-closed.
    fn reap_direct_child(&mut self, child: &mut Child) -> io::Result<DirectChildReap>;

    /// Verify recursive emptiness independently of the prior discovery.
    fn verify_recursive_empty(&mut self) -> io::Result<RecursiveEmptyProof>;

    /// Wait without busy-spinning. Fakes may override this to advance a
    /// deterministic clock.
    fn wait(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Spawn result constructed only by a prepared backend.
pub struct BackendSpawn {
    child: Child,
    kernel: Box<dyn ContainmentKernel>,
}

impl BackendSpawn {
    pub fn new(child: Child, kernel: Box<dyn ContainmentKernel>) -> Self {
        Self { child, kernel }
    }
}

/// Backend-specific resources prepared before any payload process exists.
pub trait PreparedContainmentBackend: Send {
    fn kind(&self) -> ContainmentBackendKind;
    fn root_identity(&self) -> ContainmentRootIdentity;

    /// Spawn with containment established before the payload's first
    /// instruction. Implementations must not implement this as spawn-then-attach.
    fn spawn_precontained(self: Box<Self>, command: ContainmentCommand)
    -> io::Result<BackendSpawn>;
}

/// Injectable backend selector/preparer. Selection is scoped to a factory
/// instance, avoiding process-global environment races in tests.
pub trait ContainmentBackendFactory: Send + Sync {
    fn prepare_backend(
        &self,
        policy: ContainmentBackendPolicy,
        spec: &ContainmentSpec,
    ) -> io::Result<Box<dyn PreparedContainmentBackend>>;

    /// Capability and selection evidence for the backend just returned by
    /// [`Self::prepare_backend`]. Factories without a capability probe use the
    /// default `None`.
    fn capability_diagnostic(
        &self,
        _selected_backend: ContainmentBackendKind,
    ) -> Option<ContainmentCapabilityDiagnostic> {
        None
    }
}

/// Instance-scoped entry point for prepared containment.
#[derive(Clone)]
pub struct ContainmentFactory {
    policy: ContainmentBackendPolicy,
    backend_factory: Arc<dyn ContainmentBackendFactory>,
    observer: Arc<dyn CleanupObserver>,
}

impl ContainmentFactory {
    pub fn new(
        policy: ContainmentBackendPolicy,
        backend_factory: Arc<dyn ContainmentBackendFactory>,
    ) -> Self {
        Self {
            policy,
            backend_factory,
            observer: Arc::new(NoopCleanupObserver),
        }
    }

    pub fn with_observer(mut self, observer: Arc<dyn CleanupObserver>) -> Self {
        self.observer = observer;
        self
    }

    /// Adds an observer without discarding one already installed by a
    /// composition root or deterministic test.
    pub fn with_additional_observer(mut self, observer: Arc<dyn CleanupObserver>) -> Self {
        self.observer = Arc::new(CompositeCleanupObserver {
            observers: [Arc::clone(&self.observer), observer],
        });
        self
    }

    pub fn policy(&self) -> ContainmentBackendPolicy {
        self.policy
    }

    /// Prepare all fallible ownership resources before spawning is possible.
    pub fn prepare(&self, spec: ContainmentSpec) -> io::Result<PreparedContainment> {
        let backend = self.backend_factory.prepare_backend(self.policy, &spec)?;
        let kind = backend.kind();
        if !self.policy.accepts(kind) {
            return Err(io::Error::other(format!(
                "backend {kind:?} does not satisfy policy {:?}",
                self.policy
            )));
        }
        let root = backend.root_identity();
        if root.backend() != kind {
            return Err(io::Error::other(
                "prepared backend root identity has a mismatched backend kind",
            ));
        }
        if let Some(diagnostic) = self.backend_factory.capability_diagnostic(kind) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.observer.observe_capability(&diagnostic);
            }));
            if let Some(reason) = diagnostic.fallback_reason() {
                let fallback = ContainmentFallbackObservation::new(
                    spec.identity.clone(),
                    spec.scope.clone(),
                    kind,
                    root.clone(),
                    reason,
                );
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.observer.observe_fallback(&fallback);
                }));
            }
        }
        Ok(PreparedContainment {
            backend: Some(backend),
            spec,
            observer: Arc::clone(&self.observer),
        })
    }
}

struct CompositeCleanupObserver {
    observers: [Arc<dyn CleanupObserver>; 2],
}

impl CleanupObserver for CompositeCleanupObserver {
    fn observe(&self, snapshot: &CleanupSnapshot) {
        for observer in &self.observers {
            observer.observe(snapshot);
        }
    }

    fn observe_cleanup(&self, observation: &CleanupObservation) {
        for observer in &self.observers {
            observer.observe_cleanup(observation);
        }
    }

    fn observe_capability(&self, diagnostic: &ContainmentCapabilityDiagnostic) {
        for observer in &self.observers {
            observer.observe_capability(diagnostic);
        }
    }

    fn observe_fallback(&self, fallback: &ContainmentFallbackObservation) {
        for observer in &self.observers {
            observer.observe_fallback(fallback);
        }
    }
}

/// Prepared ownership boundary. There is intentionally no attach operation.
pub struct PreparedContainment {
    backend: Option<Box<dyn PreparedContainmentBackend>>,
    spec: ContainmentSpec,
    observer: Arc<dyn CleanupObserver>,
}

impl PreparedContainment {
    pub fn backend_kind(&self) -> ContainmentBackendKind {
        self.backend
            .as_ref()
            .expect("prepared backend is present before spawn")
            .kind()
    }

    pub fn root_identity(&self) -> ContainmentRootIdentity {
        self.backend
            .as_ref()
            .expect("prepared backend is present before spawn")
            .root_identity()
    }

    pub fn spec(&self) -> &ContainmentSpec {
        &self.spec
    }

    /// Move the full command into the prepared backend. A returned process has
    /// already been placed in its recursive ownership boundary.
    pub fn spawn(mut self, command: ContainmentCommand) -> io::Result<ContainedProcess> {
        let prepared = self
            .backend
            .take()
            .expect("a prepared containment can spawn only once");
        let expected_kind = prepared.kind();
        let expected_root = prepared.root_identity();
        let spawned = prepared.spawn_precontained(command)?;
        let kernel_matches = spawned.kernel.backend_kind() == expected_kind
            && spawned.kernel.root_identity() == expected_root;
        let process = ContainedProcess::new(
            self.spec,
            expected_kind,
            expected_root,
            self.observer,
            spawned,
        );
        if !kernel_matches {
            // A defective injected backend must not turn validation into a
            // leak. Use the same fail-closed cleanup gate before rejecting it.
            let _ = process.cleanup(CleanupTrigger::Shutdown);
            return Err(io::Error::other(
                "spawned containment kernel does not match its prepared backend",
            ));
        }
        Ok(process)
    }
}

struct OwnedContainment {
    child: Child,
    kernel: Box<dyn ContainmentKernel>,
}

enum CoordinatorPhase {
    Ready,
    Cleaning,
    Complete(Box<CleanupReport>),
}

struct CoordinatorState {
    phase: CoordinatorPhase,
    owned: Option<OwnedContainment>,
}

struct CleanupCoordinator {
    spec: ContainmentSpec,
    backend: ContainmentBackendKind,
    root: ContainmentRootIdentity,
    observer: Arc<dyn CleanupObserver>,
    state: Mutex<CoordinatorState>,
    completion: Condvar,
}

/// A process that was contained before its first payload instruction.
///
/// Cleanup is coordinated exactly once. Concurrent callers wait for the first
/// caller's proof, and owner drop uses the same state machine with
/// [`CleanupTrigger::OwnerDrop`]. If inspection remains blocked, those callers
/// (including drop) remain pending rather than fabricating completion.
pub struct ContainedProcess {
    root_pid: u32,
    coordinator: CleanupCoordinator,
}

impl ContainedProcess {
    fn new(
        spec: ContainmentSpec,
        backend: ContainmentBackendKind,
        root: ContainmentRootIdentity,
        observer: Arc<dyn CleanupObserver>,
        spawned: BackendSpawn,
    ) -> Self {
        let root_pid = spawned.child.id();
        Self {
            root_pid,
            coordinator: CleanupCoordinator {
                spec,
                backend,
                root,
                observer,
                state: Mutex::new(CoordinatorState {
                    phase: CoordinatorPhase::Ready,
                    owned: Some(OwnedContainment {
                        child: spawned.child,
                        kernel: spawned.kernel,
                    }),
                }),
                completion: Condvar::new(),
            },
        }
    }

    pub fn id(&self) -> u32 {
        self.root_pid
    }

    pub fn backend_kind(&self) -> ContainmentBackendKind {
        self.coordinator.backend
    }

    pub fn root_identity(&self) -> &ContainmentRootIdentity {
        &self.coordinator.root
    }

    pub fn take_stdin(&self) -> io::Result<Option<ChildStdin>> {
        self.with_ready_child(|child| child.stdin.take())
    }

    pub fn take_stdout(&self) -> io::Result<Option<ChildStdout>> {
        self.with_ready_child(|child| child.stdout.take())
    }

    pub fn take_stderr(&self) -> io::Result<Option<ChildStderr>> {
        self.with_ready_child(|child| child.stderr.take())
    }

    pub fn try_wait_root(&self) -> io::Result<Option<ExitStatus>> {
        self.with_ready_child(Child::try_wait)?
    }

    pub fn wait_root(&self) -> io::Result<ExitStatus> {
        self.with_ready_child(Child::wait)?
    }

    fn with_ready_child<T>(&self, operation: impl FnOnce(&mut Child) -> T) -> io::Result<T> {
        let mut state = lock_unpoisoned(&self.coordinator.state);
        if !matches!(state.phase, CoordinatorPhase::Ready) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "direct child is owned by cleanup",
            ));
        }
        let owned = state
            .owned
            .as_mut()
            .expect("ready cleanup coordinator owns its process");
        Ok(operation(&mut owned.child))
    }

    /// Run or join the exactly-once cleanup state machine. This method returns
    /// only after recursive emptiness is proven and the direct child has
    /// terminal reap details.
    pub fn cleanup(&self, trigger: CleanupTrigger) -> CleanupReport {
        let mut state = lock_unpoisoned(&self.coordinator.state);
        loop {
            match &state.phase {
                CoordinatorPhase::Ready => {
                    let mut owned = state
                        .owned
                        .take()
                        .expect("ready cleanup coordinator owns its process");
                    state.phase = CoordinatorPhase::Cleaning;
                    drop(state);

                    let report = run_cleanup(&self.coordinator, trigger, &mut owned);
                    let mut state = lock_unpoisoned(&self.coordinator.state);
                    state.phase = CoordinatorPhase::Complete(Box::new(report.clone()));
                    self.coordinator.completion.notify_all();
                    return report;
                }
                CoordinatorPhase::Cleaning => {
                    state = wait_unpoisoned(&self.coordinator.completion, state);
                }
                CoordinatorPhase::Complete(report) => return report.as_ref().clone(),
            }
        }
    }
}

impl Drop for ContainedProcess {
    fn drop(&mut self) {
        let _ = self.cleanup(CleanupTrigger::OwnerDrop);
    }
}

struct CleanupEvidence {
    term_attempts: Vec<SignalAttempt>,
    omitted_term_attempts: usize,
    kill_attempts: Vec<SignalAttempt>,
    omitted_kill_attempts: usize,
    observed_survivors: Vec<ProcessIdentity>,
    omitted_survivors: usize,
    diagnostics: Vec<CleanupDiagnostic>,
    omitted_diagnostics: usize,
    direct_child_reap: DirectChildReap,
    disposition: CleanupDisposition,
    term_attempted: bool,
}

impl CleanupEvidence {
    fn new(root_pid: u32) -> Self {
        Self {
            term_attempts: Vec::new(),
            omitted_term_attempts: 0,
            kill_attempts: Vec::new(),
            omitted_kill_attempts: 0,
            observed_survivors: Vec::new(),
            omitted_survivors: 0,
            diagnostics: Vec::new(),
            omitted_diagnostics: 0,
            direct_child_reap: DirectChildReap::Pending { pid: root_pid },
            disposition: CleanupDisposition::AlreadyEmpty,
            term_attempted: false,
        }
    }

    fn remember_survivors(&mut self, members: &[ProcessIdentity], omitted: usize) {
        if members.is_empty() && omitted == 0 {
            return;
        }
        self.observed_survivors.clear();
        self.observed_survivors
            .extend(members.iter().take(MAX_SURVIVOR_IDENTITIES).cloned());
        self.omitted_survivors =
            omitted.saturating_add(members.len().saturating_sub(MAX_SURVIVOR_IDENTITIES));
    }

    fn remember_diagnostic(&mut self, diagnostic: CleanupDiagnostic) {
        if self.diagnostics.len() < MAX_CLEANUP_DIAGNOSTICS {
            self.diagnostics.push(diagnostic);
        } else {
            self.omitted_diagnostics = self.omitted_diagnostics.saturating_add(1);
        }
    }

    fn remember_batch(&mut self, signal: ContainmentSignal, batch: &SignalBatch) {
        let (attempts, omitted) = match signal {
            ContainmentSignal::Term => (&mut self.term_attempts, &mut self.omitted_term_attempts),
            ContainmentSignal::Kill => (&mut self.kill_attempts, &mut self.omitted_kill_attempts),
        };
        *omitted = omitted.saturating_add(batch.omitted());
        let remaining = MAX_SIGNAL_ATTEMPTS.saturating_sub(attempts.len());
        attempts.extend(batch.attempts().iter().take(remaining).cloned());
        *omitted = omitted.saturating_add(batch.attempts().len().saturating_sub(remaining));
    }
}

fn run_cleanup(
    coordinator: &CleanupCoordinator,
    trigger: CleanupTrigger,
    owned: &mut OwnedContainment,
) -> CleanupReport {
    let mut evidence = CleanupEvidence::new(owned.child.id());

    loop {
        observe(
            coordinator,
            CleanupSnapshot::Inspecting {
                trigger,
                phase: CleanupPhase::Discover,
            },
        );
        let discovery = match owned.kernel.discover_members() {
            Ok(discovery) => discovery,
            Err(error) => {
                block(
                    coordinator,
                    trigger,
                    CleanupPhase::Discover,
                    error,
                    &mut evidence,
                    owned.kernel.as_mut(),
                );
                continue;
            }
        };
        collect_backend_signal_evidence(owned.kernel.as_mut(), &mut evidence);
        evidence.remember_survivors(discovery.members(), discovery.omitted());

        observe(
            coordinator,
            CleanupSnapshot::Inspecting {
                trigger,
                phase: CleanupPhase::Reap,
            },
        );
        match owned.kernel.reap_direct_child(&mut owned.child) {
            Ok(reap) => evidence.direct_child_reap = reap,
            Err(error) => {
                block(
                    coordinator,
                    trigger,
                    CleanupPhase::Reap,
                    error,
                    &mut evidence,
                    owned.kernel.as_mut(),
                );
                continue;
            }
        }

        let mut has_members = !discovery.is_empty();
        if !has_members {
            observe(
                coordinator,
                CleanupSnapshot::Inspecting {
                    trigger,
                    phase: CleanupPhase::VerifyEmpty,
                },
            );
            match owned.kernel.verify_recursive_empty() {
                Ok(RecursiveEmptyProof::Proven { inspections }) => {
                    if evidence.direct_child_reap.is_terminal() {
                        let report = CleanupReport {
                            backend: coordinator.backend,
                            root: coordinator.root.clone(),
                            trigger,
                            disposition: evidence.disposition,
                            term_attempts: evidence.term_attempts,
                            omitted_term_attempts: evidence.omitted_term_attempts,
                            kill_attempts: evidence.kill_attempts,
                            omitted_kill_attempts: evidence.omitted_kill_attempts,
                            direct_child_reap: evidence.direct_child_reap,
                            recursive_empty: RecursiveEmptyProof::Proven { inspections },
                            observed_survivors: evidence.observed_survivors,
                            omitted_survivors: evidence.omitted_survivors,
                            blocked_diagnostics: evidence.diagnostics,
                            omitted_blocked_diagnostics: evidence.omitted_diagnostics,
                        };
                        observe(
                            coordinator,
                            CleanupSnapshot::Completed {
                                report: report.clone(),
                            },
                        );
                        return report;
                    }
                    block(
                        coordinator,
                        trigger,
                        CleanupPhase::Reap,
                        io::Error::other(
                            "recursive containment is empty but direct child reap is pending",
                        ),
                        &mut evidence,
                        owned.kernel.as_mut(),
                    );
                    continue;
                }
                Ok(RecursiveEmptyProof::NotEmpty { survivors, omitted }) => {
                    evidence.remember_survivors(&survivors, omitted);
                    has_members = true;
                }
                Err(error) => {
                    block(
                        coordinator,
                        trigger,
                        CleanupPhase::VerifyEmpty,
                        error,
                        &mut evidence,
                        owned.kernel.as_mut(),
                    );
                    continue;
                }
            }
        }

        if has_members && !evidence.term_attempted {
            observe(
                coordinator,
                CleanupSnapshot::Inspecting {
                    trigger,
                    phase: CleanupPhase::Term,
                },
            );
            let batch = match owned.kernel.signal_members(ContainmentSignal::Term) {
                Ok(batch) => batch,
                Err(error) => {
                    block(
                        coordinator,
                        trigger,
                        CleanupPhase::Term,
                        error,
                        &mut evidence,
                        owned.kernel.as_mut(),
                    );
                    continue;
                }
            };
            evidence.remember_batch(ContainmentSignal::Term, &batch);
            evidence.term_attempted = true;
            evidence.disposition = CleanupDisposition::Terminated;
            observe(
                coordinator,
                CleanupSnapshot::SignalAttempted {
                    trigger,
                    signal: ContainmentSignal::Term,
                    attempts: batch.attempts,
                    omitted: batch.omitted,
                },
            );
            observe(
                coordinator,
                CleanupSnapshot::GracePeriod {
                    trigger,
                    duration: coordinator.spec.term_grace,
                },
            );
            owned.kernel.wait(coordinator.spec.term_grace);
            continue;
        }

        if has_members {
            observe(
                coordinator,
                CleanupSnapshot::Inspecting {
                    trigger,
                    phase: CleanupPhase::Kill,
                },
            );
            let batch = match owned.kernel.signal_members(ContainmentSignal::Kill) {
                Ok(batch) => batch,
                Err(error) => {
                    block(
                        coordinator,
                        trigger,
                        CleanupPhase::Kill,
                        error,
                        &mut evidence,
                        owned.kernel.as_mut(),
                    );
                    continue;
                }
            };
            evidence.remember_batch(ContainmentSignal::Kill, &batch);
            evidence.disposition = CleanupDisposition::Killed;
            observe(
                coordinator,
                CleanupSnapshot::SignalAttempted {
                    trigger,
                    signal: ContainmentSignal::Kill,
                    attempts: batch.attempts,
                    omitted: batch.omitted,
                },
            );
            owned.kernel.wait(coordinator.spec.inspection_retry);
        }
    }
}

fn block(
    coordinator: &CleanupCoordinator,
    trigger: CleanupTrigger,
    phase: CleanupPhase,
    error: io::Error,
    evidence: &mut CleanupEvidence,
    kernel: &mut dyn ContainmentKernel,
) {
    let message = bounded_text(error.to_string(), MAX_DIAGNOSTIC_TEXT_BYTES);
    evidence.remember_diagnostic(CleanupDiagnostic::new(phase, message.clone()));
    observe(
        coordinator,
        CleanupSnapshot::Blocked {
            trigger,
            phase,
            message,
            survivors: evidence.observed_survivors.clone(),
            omitted_survivors: evidence.omitted_survivors,
        },
    );
    kernel.wait(coordinator.spec.inspection_retry);
}

fn observe(coordinator: &CleanupCoordinator, snapshot: CleanupSnapshot) {
    let observation = CleanupObservation::new(
        coordinator.spec.identity.clone(),
        coordinator.spec.scope.clone(),
        coordinator.backend,
        coordinator.root.clone(),
        snapshot,
    );
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        coordinator.observer.observe_cleanup(&observation);
    }));
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn wait_unpoisoned<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar
        .wait(guard)
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
