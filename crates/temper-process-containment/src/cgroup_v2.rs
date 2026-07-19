//! Delegated Linux cgroup-v2 containment.
//!
//! The backend creates the ownership cgroup before `fork`, opens its controls,
//! and writes `0` to `cgroup.procs` from `CommandExt::pre_exec`.  The payload
//! therefore cannot execute outside its cgroup.  A directory descriptor for
//! that cgroup is inherited at [`INHERITED_CGROUP_SCOPE_FD`], allowing a nested
//! Temper process to prepare tool cgroups below the job boundary without
//! consulting process-global environment state.

use std::io;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod containment;
mod ownership;
mod platform;
mod process;

pub use containment::CgroupV2Containment;
use containment::{CgroupV2PreparedContainment, PreparedControls};
pub use ownership::{CgroupV2ScavengeReport, RetainedStaleCgroup};
use platform::*;
use process::*;

use crate::{
    BackendSpawn, ContainmentBackendFactory, ContainmentBackendKind, ContainmentBackendPolicy,
    ContainmentCommand, ContainmentKernel, ContainmentRootIdentity, ContainmentScope,
    ContainmentSignal, ContainmentSpec, DirectChildReap, MemberDiscovery,
    PreparedContainmentBackend, ProcessIdentity, RecursiveEmptyProof, SignalAttempt,
    SignalAttemptOutcome, SignalBatch,
};

/// Descriptor used to pass a job's cgroup directory to an out-of-process
/// Temper agent.  It is deliberately fixed rather than communicated through a
/// mutable global environment variable.
pub const INHERITED_CGROUP_SCOPE_FD: RawFd = 198;

const DEFAULT_SUBTREE: &str = "temper";
const MAX_SCAVENGE_DIAGNOSTICS: usize = 128;
const MAX_SCAVENGE_DIAGNOSTIC_BYTES: usize = 2 * 1024;
const ROLLBACK_RETRIES: usize = 50;
const ROLLBACK_RETRY: Duration = Duration::from_millis(10);

/// Deterministic path context for cgroups owned by one factory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CgroupV2FactoryConfig {
    owner: String,
    owner_pid: u32,
    owner_start_time: u64,
    job: String,
    attempt: String,
    subtree: String,
}

impl CgroupV2FactoryConfig {
    pub fn new(job: impl AsRef<str>, attempt: impl AsRef<str>) -> io::Result<Self> {
        Self::for_owner("process", job, attempt)
    }

    /// Bind every cgroup made by this factory to one logical owner and the
    /// current process boot. Startup scavenging uses both the PID and its
    /// kernel start-time identity before deciding that an ownership root is
    /// stale.
    pub fn for_owner(
        owner: impl AsRef<str>,
        job: impl AsRef<str>,
        attempt: impl AsRef<str>,
    ) -> io::Result<Self> {
        let process = proc_identity(std::process::id())?;
        Ok(Self {
            owner: encode_component(owner.as_ref(), "owner")?,
            owner_pid: process.pid(),
            owner_start_time: process.start_time_identity(),
            job: encode_component(job.as_ref(), "job")?,
            attempt: encode_component(attempt.as_ref(), "attempt")?,
            subtree: DEFAULT_SUBTREE.to_owned(),
        })
    }

    /// Override the dedicated Temper-owned subtree name.
    pub fn with_subtree(mut self, subtree: impl AsRef<str>) -> io::Result<Self> {
        self.subtree = encode_component(subtree.as_ref(), "subtree")?;
        Ok(self)
    }

    pub fn job(&self) -> &str {
        &self.job
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn attempt(&self) -> &str {
        &self.attempt
    }

    pub fn subtree(&self) -> &str {
        &self.subtree
    }
}

/// Result of probing the host's unified hierarchy and the current delegated
/// cgroup.  `delegation_available` is the final selection decision; individual
/// fields retain enough detail for startup diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CgroupV2Capability {
    unified_mount: Option<PathBuf>,
    delegated_subtree: Option<PathBuf>,
    dedicated_subtree: Option<PathBuf>,
    delegation: bool,
    writable_subtree: bool,
    cgroup_kill: bool,
    pidfd: bool,
    probe_rollback_complete: bool,
    diagnostic: Option<String>,
}

impl CgroupV2Capability {
    fn unavailable(diagnostic: impl Into<String>, pidfd: bool) -> Self {
        Self {
            unified_mount: None,
            delegated_subtree: None,
            dedicated_subtree: None,
            delegation: false,
            writable_subtree: false,
            cgroup_kill: false,
            pidfd,
            probe_rollback_complete: true,
            diagnostic: Some(diagnostic.into()),
        }
    }

    pub fn unified_mount(&self) -> Option<&Path> {
        self.unified_mount.as_deref()
    }

    pub fn delegated_subtree(&self) -> Option<&Path> {
        self.delegated_subtree.as_deref()
    }

    pub fn dedicated_subtree(&self) -> Option<&Path> {
        self.dedicated_subtree.as_deref()
    }

    pub fn delegation(&self) -> bool {
        self.delegation
    }

    pub fn writable_subtree(&self) -> bool {
        self.writable_subtree
    }

    pub fn cgroup_kill(&self) -> bool {
        self.cgroup_kill
    }

    pub fn pidfd(&self) -> bool {
        self.pidfd
    }

    /// Whether every temporary cgroup made by capability probing was removed.
    /// Auto-selection must not fall back while a partial probe is still owned.
    pub fn probe_rollback_complete(&self) -> bool {
        self.probe_rollback_complete
    }

    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }

    pub fn delegation_available(&self) -> bool {
        self.unified_mount.is_some()
            && self.delegation
            && self.writable_subtree
            && self.pidfd
            && self.dedicated_subtree.is_some()
    }
}

/// Scoped emission seam for auto-selection capability diagnostics.
pub trait CgroupV2CapabilityObserver: Send + Sync {
    fn observe(&self, capability: &CgroupV2Capability);
}

#[derive(Debug)]
struct NoopCapabilityObserver;

impl CgroupV2CapabilityObserver for NoopCapabilityObserver {
    fn observe(&self, _capability: &CgroupV2Capability) {}
}

/// Production selector and preparer for delegated cgroup v2.
///
/// `Auto` falls back only through the explicitly supplied backend factory.  A
/// cgroup preparation failure is eligible for fallback only after every cgroup
/// created by that attempt has been rolled back.
pub struct CgroupV2BackendFactory {
    config: CgroupV2FactoryConfig,
    capability: CgroupV2Capability,
    fs: Arc<dyn CgroupFileSystem>,
    processes: Arc<dyn LinuxProcessApi>,
    fallback: Option<Arc<dyn ContainmentBackendFactory>>,
    observer: Arc<dyn CgroupV2CapabilityObserver>,
    fallback_reason: Mutex<Option<String>>,
    nonce_base: u64,
    nonce: AtomicU64,
}

#[derive(Debug)]
struct CgroupPrepareFailure {
    error: io::Error,
    rollback_complete: bool,
}

impl CgroupPrepareFailure {
    fn before_setup(error: io::Error) -> Self {
        Self {
            error,
            rollback_complete: true,
        }
    }

    fn after_setup(error: io::Error, rollback: io::Result<()>) -> Self {
        match rollback {
            Ok(()) => Self {
                error,
                rollback_complete: true,
            },
            Err(rollback) => Self {
                error: io::Error::other(format!(
                    "{error}; partial cgroup rollback failed: {rollback}"
                )),
                rollback_complete: false,
            },
        }
    }

    fn into_io_error(self) -> io::Error {
        self.error
    }
}

impl CgroupV2BackendFactory {
    /// Probe the real host.  If a valid inherited scope descriptor is present,
    /// it is preferred over `/proc/self/cgroup`, ensuring nested tool cgroups
    /// are created under their out-of-process job cgroup.
    pub fn system(config: CgroupV2FactoryConfig) -> Self {
        let fs: Arc<dyn CgroupFileSystem> = Arc::new(RealCgroupFileSystem);
        let processes: Arc<dyn LinuxProcessApi> = Arc::new(RealLinuxProcessApi);
        let capability = probe_system(&config, fs.as_ref(), processes.as_ref());
        Self::from_parts(config, capability, fs, processes)
    }

    fn from_parts(
        config: CgroupV2FactoryConfig,
        capability: CgroupV2Capability,
        fs: Arc<dyn CgroupFileSystem>,
        processes: Arc<dyn LinuxProcessApi>,
    ) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let folded = u64::try_from(timestamp).unwrap_or_else(|_| {
            let high = u64::try_from(timestamp >> 64).unwrap_or_default();
            let low = timestamp as u64;
            high ^ low
        });
        Self {
            config,
            capability,
            fs,
            processes,
            fallback: None,
            observer: Arc::new(NoopCapabilityObserver),
            fallback_reason: Mutex::new(None),
            nonce_base: folded ^ u64::from(std::process::id()).rotate_left(17),
            nonce: AtomicU64::new(0),
        }
    }

    pub fn with_fallback(mut self, fallback: Arc<dyn ContainmentBackendFactory>) -> Self {
        self.fallback = Some(fallback);
        self
    }

    pub fn with_capability_observer(
        mut self,
        observer: Arc<dyn CgroupV2CapabilityObserver>,
    ) -> Self {
        self.observer = observer;
        self
    }

    pub fn capability(&self) -> &CgroupV2Capability {
        &self.capability
    }

    fn prepare_cgroup(
        &self,
        spec: &ContainmentSpec,
    ) -> Result<Box<dyn PreparedContainmentBackend>, CgroupPrepareFailure> {
        let dedicated = self.capability.dedicated_subtree().ok_or_else(|| {
            CgroupPrepareFailure::before_setup(unavailable_error(&self.capability))
        })?;
        let owner_kind =
            scope_component(&spec.scope).map_err(CgroupPrepareFailure::before_setup)?;
        let owner_id = encode_component(spec.identity.as_str(), "owner identity")
            .map_err(CgroupPrepareFailure::before_setup)?;
        let components = [
            format!("worker-{}", self.config.owner),
            format!(
                "boot-{}-{}",
                self.config.owner_pid, self.config.owner_start_time
            ),
            format!("job-{}", self.config.job),
            format!("attempt-{}", self.config.attempt),
            format!("owner-kind-{owner_kind}"),
            format!("owner-id-{owner_id}"),
        ];

        let mut path = dedicated.to_path_buf();
        let mut created = Vec::new();
        for component in components {
            path.push(component);
            if self.fs.exists(&path) {
                continue;
            }
            match self.fs.create_cgroup(&path) {
                Ok(()) => created.push(path.clone()),
                // Deterministic hierarchy components are intentionally shared
                // by concurrent owners. Losing a mkdir race is not a partial
                // setup failure and must not remove the winner's directory.
                Err(error)
                    if error.kind() == io::ErrorKind::AlreadyExists && self.fs.exists(&path) => {}
                Err(error) => {
                    // A filesystem implementation may have created the
                    // directory before reporting a control initialization
                    // failure.
                    if self.fs.exists(&path) {
                        created.push(path.clone());
                    }
                    return Err(CgroupPrepareFailure::after_setup(
                        io::Error::new(
                            error.kind(),
                            format!("create cgroup {} failed: {error}", path.display()),
                        ),
                        rollback_created(self.fs.as_ref(), &created),
                    ));
                }
            }
        }

        let owner_path = path;
        let path = (0..128)
            .find_map(|_| {
                let nonce = self.nonce.fetch_add(1, Ordering::Relaxed);
                let nonce = format!("{:016x}-{:016x}", self.nonce_base, nonce);
                let candidate = owner_path.join(format!("nonce-{nonce}"));
                match self.fs.create_cgroup(&candidate) {
                    Ok(()) => {
                        created.push(candidate.clone());
                        Some(Ok(candidate))
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => None,
                    Err(error) => {
                        if self.fs.exists(&candidate) {
                            created.push(candidate.clone());
                        }
                        Some(Err(CgroupPrepareFailure::after_setup(
                            io::Error::new(
                                error.kind(),
                                format!(
                                    "create unique cgroup {} failed: {error}",
                                    candidate.display()
                                ),
                            ),
                            rollback_created(self.fs.as_ref(), &created),
                        )))
                    }
                }
            })
            .unwrap_or_else(|| {
                Err(CgroupPrepareFailure::after_setup(
                    io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "could not allocate a unique cgroup nonce after 128 attempts",
                    ),
                    rollback_created(self.fs.as_ref(), &created),
                ))
            })?;

        let controls = match PreparedControls::open(self.fs.as_ref(), path.clone()) {
            Ok(controls) => controls,
            Err(error) => {
                return Err(CgroupPrepareFailure::after_setup(
                    io::Error::new(
                        error.kind(),
                        format!("open controls for {} failed: {error}", path.display()),
                    ),
                    rollback_created(self.fs.as_ref(), &created),
                ));
            }
        };
        let root = ContainmentRootIdentity::new(
            ContainmentBackendKind::LinuxCgroupV2,
            path.to_string_lossy(),
        );
        Ok(Box::new(CgroupV2PreparedContainment {
            controls: Some(controls),
            root,
            fs: Arc::clone(&self.fs),
            processes: Arc::clone(&self.processes),
        }))
    }

    fn fallback(&self, spec: &ContainmentSpec) -> io::Result<Box<dyn PreparedContainmentBackend>> {
        let fallback = self.fallback.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "delegated cgroup v2 unavailable and no fallback was supplied: {}",
                    self.capability
                        .diagnostic()
                        .unwrap_or("capability requirements not met")
                ),
            )
        })?;
        fallback.prepare_backend(ContainmentBackendPolicy::ForceLinuxSupervisor, spec)
    }
}

impl ContainmentBackendFactory for CgroupV2BackendFactory {
    fn prepare_backend(
        &self,
        policy: ContainmentBackendPolicy,
        spec: &ContainmentSpec,
    ) -> io::Result<Box<dyn PreparedContainmentBackend>> {
        if policy == ContainmentBackendPolicy::Auto {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.observer.observe(&self.capability);
            }));
        }
        match policy {
            ContainmentBackendPolicy::RequireCgroupV2 => {
                if !self.capability.delegation_available() {
                    return Err(unavailable_error(&self.capability));
                }
                self.prepare_cgroup(spec)
                    .map_err(CgroupPrepareFailure::into_io_error)
            }
            ContainmentBackendPolicy::Auto => {
                if !self.capability.delegation_available() {
                    if !self.capability.probe_rollback_complete() {
                        return Err(unavailable_error(&self.capability));
                    }
                    *self
                        .fallback_reason
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(
                        self.capability
                            .diagnostic()
                            .unwrap_or("delegated cgroup-v2 capability requirements were not met")
                            .to_string(),
                    );
                    return self.fallback(spec);
                }
                match self.prepare_cgroup(spec) {
                    Ok(prepared) => {
                        *self
                            .fallback_reason
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                        Ok(prepared)
                    }
                    Err(error) if error.rollback_complete => {
                        let cgroup_error = error.into_io_error();
                        *self
                            .fallback_reason
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some(format!("cgroup preparation failed: {cgroup_error}"));
                        self.fallback(spec).map_err(|fallback| {
                            io::Error::other(format!(
                                "cgroup preparation failed: {cgroup_error}; fallback failed: {fallback}"
                            ))
                        })
                    }
                    Err(error) => Err(error.into_io_error()),
                }
            }
            ContainmentBackendPolicy::ForceLinuxSupervisor
            | ContainmentBackendPolicy::RequireWindowsJob => self
                .fallback
                .as_ref()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::Unsupported,
                        "requested backend is not supplied by the cgroup-v2 factory",
                    )
                })?
                .prepare_backend(policy, spec),
        }
    }

    fn capability_diagnostic(
        &self,
        selected_backend: ContainmentBackendKind,
    ) -> Option<crate::ContainmentCapabilityDiagnostic> {
        let fallback_reason =
            (selected_backend != ContainmentBackendKind::LinuxCgroupV2).then(|| {
                self.fallback_reason
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .or_else(|| self.capability.diagnostic().map(str::to_string))
                    .unwrap_or_else(|| {
                        "delegated cgroup-v2 capability requirements were not met".to_string()
                    })
            });
        Some(crate::ContainmentCapabilityDiagnostic::new(
            self.capability
                .unified_mount()
                .map(|path| path.to_string_lossy().into_owned()),
            self.capability.delegation(),
            self.capability.writable_subtree(),
            self.capability.cgroup_kill(),
            self.capability.pidfd(),
            selected_backend,
            fallback_reason,
        ))
    }
}

#[cfg(test)]
mod tests;
