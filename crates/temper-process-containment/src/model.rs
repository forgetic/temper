use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Maximum number of process identities retained in a cleanup diagnostic.
pub const MAX_SURVIVOR_IDENTITIES: usize = 64;
/// Maximum number of TERM or KILL attempts retained in a terminal report.
pub const MAX_SIGNAL_ATTEMPTS: usize = 128;
/// Maximum number of inspection failures retained in a terminal report.
pub const MAX_CLEANUP_DIAGNOSTICS: usize = 32;
/// Maximum UTF-8 bytes retained for one cleanup diagnostic.
pub const MAX_DIAGNOSTIC_TEXT_BYTES: usize = 2 * 1024;
/// Maximum UTF-8 bytes retained for a process executable diagnostic.
pub const MAX_EXECUTABLE_IDENTITY_BYTES: usize = 4 * 1024;
/// Maximum UTF-8 bytes accepted for a logical containment identity.
pub const MAX_CONTAINMENT_IDENTITY_BYTES: usize = 256;
/// Maximum UTF-8 bytes retained for an implementation-specific root identity.
pub const MAX_ROOT_IDENTITY_BYTES: usize = 4 * 1024;

/// Selection requested for one [`crate::ContainmentFactory`] instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainmentBackendPolicy {
    /// Prefer delegated cgroup v2 and otherwise use a descendant-complete
    /// platform fallback.
    Auto,
    /// Preparing containment must fail unless delegated cgroup v2 is selected.
    RequireCgroupV2,
    /// Select the Linux supervisor even when delegated cgroup v2 is available.
    ForceLinuxSupervisor,
    /// Preparing containment must fail unless a race-free Windows Job is used.
    RequireWindowsJob,
}

/// A descendant-complete production backend.
///
/// The old Unix process-group adapter deliberately has no variant here, so a
/// backend selected by the prepared contract cannot accidentally claim that a
/// process group is descendant-complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainmentBackendKind {
    LinuxCgroupV2,
    LinuxSupervisor,
    WindowsJob,
}

impl ContainmentBackendPolicy {
    pub(crate) fn accepts(self, kind: ContainmentBackendKind) -> bool {
        match self {
            Self::Auto => true,
            Self::RequireCgroupV2 => kind == ContainmentBackendKind::LinuxCgroupV2,
            Self::ForceLinuxSupervisor => kind == ContainmentBackendKind::LinuxSupervisor,
            Self::RequireWindowsJob => kind == ContainmentBackendKind::WindowsJob,
        }
    }
}

/// Stable logical identity assigned by the process owner before preparation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ContainmentIdentity(String);

impl ContainmentIdentity {
    pub fn new(value: impl Into<String>) -> io::Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "containment identity must not be empty",
            ));
        }
        if value.len() > MAX_CONTAINMENT_IDENTITY_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("containment identity exceeds {MAX_CONTAINMENT_IDENTITY_BYTES} bytes"),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The owner boundary protected by a containment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContainmentScope {
    Job,
    Tool,
    Agent,
    McpServer,
    WorkerCommand,
    PrePush,
    Custom(String),
}

/// Preparation and cleanup timing for one containment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainmentSpec {
    pub identity: ContainmentIdentity,
    pub scope: ContainmentScope,
    /// Time between a successful TERM attempt and the first KILL attempt.
    pub term_grace: Duration,
    /// Minimum delay between blocked inspections or repeated KILL attempts.
    pub inspection_retry: Duration,
}

impl ContainmentSpec {
    pub fn new(identity: ContainmentIdentity, scope: ContainmentScope) -> Self {
        Self {
            identity,
            scope,
            term_grace: Duration::from_secs(2),
            inspection_retry: Duration::from_millis(100),
        }
    }

    pub fn with_timing(mut self, term_grace: Duration, inspection_retry: Duration) -> Self {
        self.term_grace = term_grace;
        self.inspection_retry = inspection_retry;
        self
    }
}

/// Stable backend root used in diagnostics and emptiness proofs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainmentRootIdentity {
    backend: ContainmentBackendKind,
    value: String,
}

impl ContainmentRootIdentity {
    pub fn new(backend: ContainmentBackendKind, value: impl Into<String>) -> Self {
        Self {
            backend,
            value: bounded_text(value.into(), MAX_ROOT_IDENTITY_BYTES),
        }
    }

    pub fn backend(&self) -> ContainmentBackendKind {
        self.backend
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

/// PID identity stable across PID reuse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessIdentity {
    pid: u32,
    ppid: u32,
    process_group_id: u32,
    session_id: u32,
    start_time_identity: u64,
    executable: PathBuf,
}

impl ProcessIdentity {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pid: u32,
        ppid: u32,
        process_group_id: u32,
        session_id: u32,
        start_time_identity: u64,
        executable: impl Into<PathBuf>,
    ) -> Self {
        let executable = executable.into();
        let executable = bounded_path(executable, MAX_EXECUTABLE_IDENTITY_BYTES);
        Self {
            pid,
            ppid,
            process_group_id,
            session_id,
            start_time_identity,
            executable,
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn ppid(&self) -> u32 {
        self.ppid
    }

    pub fn process_group_id(&self) -> u32 {
        self.process_group_id
    }

    pub fn session_id(&self) -> u32 {
        self.session_id
    }

    pub fn start_time_identity(&self) -> u64 {
        self.start_time_identity
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }
}

/// Signal requested by the common cleanup state machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainmentSignal {
    Term,
    Kill,
}

/// Result of signaling a process identity, including PID-reuse protection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignalAttemptOutcome {
    Succeeded,
    ProcessGone,
    PidReused,
    Failed(String),
}

/// One bounded, structured signal diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignalAttempt {
    process: ProcessIdentity,
    signal: ContainmentSignal,
    outcome: SignalAttemptOutcome,
}

impl SignalAttempt {
    pub fn succeeded(process: ProcessIdentity, signal: ContainmentSignal) -> Self {
        Self {
            process,
            signal,
            outcome: SignalAttemptOutcome::Succeeded,
        }
    }

    pub fn process_gone(process: ProcessIdentity, signal: ContainmentSignal) -> Self {
        Self {
            process,
            signal,
            outcome: SignalAttemptOutcome::ProcessGone,
        }
    }

    pub fn pid_reused(process: ProcessIdentity, signal: ContainmentSignal) -> Self {
        Self {
            process,
            signal,
            outcome: SignalAttemptOutcome::PidReused,
        }
    }

    pub fn failed(
        process: ProcessIdentity,
        signal: ContainmentSignal,
        error: impl Into<String>,
    ) -> Self {
        Self {
            process,
            signal,
            outcome: SignalAttemptOutcome::Failed(bounded_text(
                error.into(),
                MAX_DIAGNOSTIC_TEXT_BYTES,
            )),
        }
    }

    pub fn process(&self) -> &ProcessIdentity {
        &self.process
    }

    pub fn signal(&self) -> ContainmentSignal {
        self.signal
    }

    pub fn outcome(&self) -> &SignalAttemptOutcome {
        &self.outcome
    }
}

/// A bounded batch returned after the backend has attempted to signal every
/// currently owned member. `omitted` counts attempts not retained for
/// diagnostics; it never means those members were skipped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignalBatch {
    pub(crate) attempts: Vec<SignalAttempt>,
    pub(crate) omitted: usize,
}

impl SignalBatch {
    pub fn new(mut attempts: Vec<SignalAttempt>, omitted: usize) -> Self {
        let overflow = attempts.len().saturating_sub(MAX_SIGNAL_ATTEMPTS);
        attempts.truncate(MAX_SIGNAL_ATTEMPTS);
        Self {
            attempts,
            omitted: omitted.saturating_add(overflow),
        }
    }

    pub fn attempts(&self) -> &[SignalAttempt] {
        &self.attempts
    }

    pub fn omitted(&self) -> usize {
        self.omitted
    }
}

/// A bounded membership sample. Backends must inspect the complete ownership
/// boundary even though only this many identities are retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberDiscovery {
    members: Vec<ProcessIdentity>,
    omitted: usize,
}

impl MemberDiscovery {
    pub fn new(mut members: Vec<ProcessIdentity>, omitted: usize) -> Self {
        let overflow = members.len().saturating_sub(MAX_SURVIVOR_IDENTITIES);
        members.truncate(MAX_SURVIVOR_IDENTITIES);
        Self {
            members,
            omitted: omitted.saturating_add(overflow),
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new(), 0)
    }

    pub fn members(&self) -> &[ProcessIdentity] {
        &self.members
    }

    pub fn omitted(&self) -> usize {
        self.omitted
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty() && self.omitted == 0
    }
}

/// Direct-child wait/reap detail required by every terminal report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectChildReap {
    Pending { pid: u32 },
    Reaped { pid: u32, exit_code: Option<i32> },
    AlreadyReaped { pid: u32, exit_code: Option<i32> },
}

impl DirectChildReap {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Pending { .. })
    }
}

/// Recursive ownership-boundary verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecursiveEmptyProof {
    Proven {
        inspections: u64,
    },
    NotEmpty {
        survivors: Vec<ProcessIdentity>,
        omitted: usize,
    },
}

impl RecursiveEmptyProof {
    pub fn proven(inspections: u64) -> Self {
        Self::Proven { inspections }
    }

    pub fn not_empty(mut survivors: Vec<ProcessIdentity>, omitted: usize) -> Self {
        let overflow = survivors.len().saturating_sub(MAX_SURVIVOR_IDENTITIES);
        survivors.truncate(MAX_SURVIVOR_IDENTITIES);
        Self::NotEmpty {
            survivors,
            omitted: omitted.saturating_add(overflow),
        }
    }
}

/// Why cleanup began. The first trigger wins; all later callers receive the
/// same terminal report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupTrigger {
    NormalRootExit,
    Timeout,
    Cancellation,
    OwnerDrop,
    Watchdog,
    Shutdown,
}

/// How recursive emptiness was reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupDisposition {
    AlreadyEmpty,
    Terminated,
    Killed,
}

/// State-machine phase associated with an inspection failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupPhase {
    Discover,
    Term,
    Grace,
    Kill,
    Reap,
    VerifyEmpty,
}

/// A bounded inspection diagnostic retained after recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupDiagnostic {
    phase: CleanupPhase,
    message: String,
}

impl CleanupDiagnostic {
    pub(crate) fn new(phase: CleanupPhase, message: impl Into<String>) -> Self {
        Self {
            phase,
            message: bounded_text(message.into(), MAX_DIAGNOSTIC_TEXT_BYTES),
        }
    }

    pub fn phase(&self) -> CleanupPhase {
        self.phase
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Terminal cleanup evidence. Construction is private so a report cannot exist
/// without [`RecursiveEmptyProof::Proven`] and terminal direct-child reap
/// details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupReport {
    pub(crate) backend: ContainmentBackendKind,
    pub(crate) root: ContainmentRootIdentity,
    pub(crate) trigger: CleanupTrigger,
    pub(crate) disposition: CleanupDisposition,
    pub(crate) term_attempts: Vec<SignalAttempt>,
    pub(crate) omitted_term_attempts: usize,
    pub(crate) kill_attempts: Vec<SignalAttempt>,
    pub(crate) omitted_kill_attempts: usize,
    pub(crate) direct_child_reap: DirectChildReap,
    pub(crate) recursive_empty: RecursiveEmptyProof,
    pub(crate) observed_survivors: Vec<ProcessIdentity>,
    pub(crate) omitted_survivors: usize,
    pub(crate) blocked_diagnostics: Vec<CleanupDiagnostic>,
    pub(crate) omitted_blocked_diagnostics: usize,
}

impl CleanupReport {
    pub fn backend(&self) -> ContainmentBackendKind {
        self.backend
    }

    pub fn root(&self) -> &ContainmentRootIdentity {
        &self.root
    }

    pub fn trigger(&self) -> CleanupTrigger {
        self.trigger
    }

    pub fn disposition(&self) -> CleanupDisposition {
        self.disposition
    }

    pub fn term_attempts(&self) -> &[SignalAttempt] {
        &self.term_attempts
    }

    pub fn omitted_term_attempts(&self) -> usize {
        self.omitted_term_attempts
    }

    pub fn kill_attempts(&self) -> &[SignalAttempt] {
        &self.kill_attempts
    }

    pub fn omitted_kill_attempts(&self) -> usize {
        self.omitted_kill_attempts
    }

    pub fn direct_child_reap(&self) -> &DirectChildReap {
        &self.direct_child_reap
    }

    pub fn recursive_empty(&self) -> &RecursiveEmptyProof {
        &self.recursive_empty
    }

    pub fn observed_survivors(&self) -> &[ProcessIdentity] {
        &self.observed_survivors
    }

    pub fn omitted_survivors(&self) -> usize {
        self.omitted_survivors
    }

    pub fn blocked_diagnostics(&self) -> &[CleanupDiagnostic] {
        &self.blocked_diagnostics
    }

    pub fn omitted_blocked_diagnostics(&self) -> usize {
        self.omitted_blocked_diagnostics
    }
}

/// Observer-visible progress. `Blocked` is deliberately non-terminal: no
/// `CleanupReport` is present and the cleanup caller remains pending.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CleanupSnapshot {
    Inspecting {
        trigger: CleanupTrigger,
        phase: CleanupPhase,
    },
    SignalAttempted {
        trigger: CleanupTrigger,
        signal: ContainmentSignal,
        attempts: Vec<SignalAttempt>,
        omitted: usize,
    },
    GracePeriod {
        trigger: CleanupTrigger,
        duration: Duration,
    },
    Blocked {
        trigger: CleanupTrigger,
        phase: CleanupPhase,
        message: String,
        survivors: Vec<ProcessIdentity>,
        omitted_survivors: usize,
    },
    Completed {
        report: CleanupReport,
    },
}

/// Per-factory observer seam. Implementations should return promptly; observer
/// panics are isolated from process cleanup.
pub trait CleanupObserver: Send + Sync {
    fn observe(&self, snapshot: &CleanupSnapshot);
}

#[derive(Debug)]
pub(crate) struct NoopCleanupObserver;

impl CleanupObserver for NoopCleanupObserver {
    fn observe(&self, _snapshot: &CleanupSnapshot) {}
}

pub(crate) fn bounded_text(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

fn bounded_path(value: PathBuf, limit: usize) -> PathBuf {
    let rendered = value.to_string_lossy().into_owned();
    PathBuf::from(bounded_text(rendered, limit))
}
