//! Agent-scoped composition for descendant-complete process containment.
//!
//! One context is created at the agent process/in-process runner boundary and
//! cloned into every process-owning tool. Backend selection is therefore
//! instance scoped (and injectable in tests) rather than controlled through
//! process-global environment variables.

#[cfg(all(target_os = "linux", debug_assertions))]
use std::io;
#[cfg(all(target_os = "linux", debug_assertions))]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use temper_process_containment::{
    ContainmentBackendFactory, ContainmentBackendPolicy, ContainmentFactory, ContainmentIdentity,
    ContainmentScope, ContainmentSpec,
};

/// Concrete containment authority shared by one agent and all of its nested
/// tools and sub-agents.
///
/// `outer_scope` records an already-established owner (normally an inherited
/// out-of-process job cgroup). The factory itself discovers that inherited
/// kernel scope; retaining the typed scope here makes the ownership topology
/// explicit as the context is threaded through agent construction.
#[derive(Clone)]
pub struct AgentContainmentContext {
    factory: ContainmentFactory,
    outer_scope: Option<ContainmentScope>,
    next_identity: Arc<AtomicU64>,
    term_grace: Duration,
    inspection_retry: Duration,
}

impl AgentContainmentContext {
    /// Builds a context from an explicitly selected factory. This is the
    /// production composition seam for standalone runners and the deterministic
    /// backend-injection seam for tests.
    pub fn new(factory: ContainmentFactory, outer_scope: Option<ContainmentScope>) -> Self {
        Self {
            factory,
            outer_scope,
            next_identity: Arc::new(AtomicU64::new(1)),
            term_grace: Duration::from_secs(2),
            inspection_retry: Duration::from_millis(100),
        }
    }

    /// Builds the platform production factory. On Linux this probes a delegated
    /// cgroup-v2 subtree (preferring the inherited job-scope descriptor) and
    /// falls back to an independently authoritative per-tool supervisor.
    pub fn production(outer_scope: Option<ContainmentScope>) -> Self {
        let backend: Arc<dyn ContainmentBackendFactory> = production_backend_factory();
        Self::new(
            ContainmentFactory::new(ContainmentBackendPolicy::Auto, backend),
            outer_scope,
        )
    }

    /// Overrides cleanup timing for this context. Production uses the default
    /// two-second TERM grace; deterministic tests use this instance-scoped seam
    /// instead of process-global environment variables.
    pub fn with_cleanup_timing(mut self, term_grace: Duration, inspection_retry: Duration) -> Self {
        self.term_grace = term_grace;
        self.inspection_retry = inspection_retry;
        self
    }

    pub fn factory(&self) -> &ContainmentFactory {
        &self.factory
    }

    pub fn outer_scope(&self) -> Option<&ContainmentScope> {
        self.outer_scope.as_ref()
    }

    /// Allocates a process-owner identity unique within this agent context.
    pub fn containment_spec(&self, owner: &str, scope: ContainmentScope) -> ContainmentSpec {
        let sequence = self.next_identity.fetch_add(1, Ordering::Relaxed);
        // `owner` originates from a bounded tool/server name in production.
        // Keep arbitrary injected labels bounded without losing uniqueness.
        let owner_budget =
            temper_process_containment::MAX_CONTAINMENT_IDENTITY_BYTES.saturating_sub(1 + 16);
        let mut owner = owner.to_string();
        if owner.len() > owner_budget {
            let mut end = owner_budget;
            while !owner.is_char_boundary(end) {
                end -= 1;
            }
            owner.truncate(end);
        }
        let identity = ContainmentIdentity::new(format!("{owner}-{sequence:016x}"))
            .and_then(|identity| identity.with_owner_identifier(owner))
            .expect("bounded non-empty agent containment identity");
        ContainmentSpec::new(identity, scope).with_timing(self.term_grace, self.inspection_retry)
    }
}

#[cfg(target_os = "linux")]
fn production_backend_factory() -> Arc<dyn ContainmentBackendFactory> {
    use temper_process_containment::{
        CgroupV2BackendFactory, CgroupV2FactoryConfig, LinuxSupervisorBackendFactory,
    };

    // Generated integration-test harnesses cannot dispatch the fallback's
    // hidden early-main protocol. Existing non-containment tests therefore get
    // an object-scoped process-group backend; the production-path containment
    // tests inject the real supervisor explicitly through this context.
    #[cfg(debug_assertions)]
    if running_under_cargo_test() {
        return Arc::new(CargoTestBackendFactory);
    }

    let process = std::process::id().to_string();
    let config = CgroupV2FactoryConfig::new(format!("agent-{process}"), "session")
        .expect("static agent cgroup components are valid");
    let supervisor: Arc<dyn ContainmentBackendFactory> =
        Arc::new(LinuxSupervisorBackendFactory::new());
    Arc::new(CgroupV2BackendFactory::system(config).with_fallback(supervisor))
}

#[cfg(all(target_os = "linux", debug_assertions))]
fn running_under_cargo_test() -> bool {
    std::env::current_exe().ok().is_some_and(|executable| {
        executable
            .parent()
            .and_then(|parent| parent.file_name())
            .is_some_and(|name| name == "deps")
    })
}

#[cfg(all(target_os = "linux", debug_assertions))]
#[derive(Debug)]
struct CargoTestBackendFactory;

#[cfg(all(target_os = "linux", debug_assertions))]
impl ContainmentBackendFactory for CargoTestBackendFactory {
    fn prepare_backend(
        &self,
        _policy: ContainmentBackendPolicy,
        spec: &ContainmentSpec,
    ) -> io::Result<Box<dyn temper_process_containment::PreparedContainmentBackend>> {
        Ok(Box::new(CargoTestPrepared {
            root: temper_process_containment::ContainmentRootIdentity::new(
                temper_process_containment::ContainmentBackendKind::LinuxSupervisor,
                format!("cargo-test:{}", spec.identity.as_str()),
            ),
        }))
    }
}

#[cfg(all(target_os = "linux", debug_assertions))]
struct CargoTestPrepared {
    root: temper_process_containment::ContainmentRootIdentity,
}

#[cfg(all(target_os = "linux", debug_assertions))]
impl temper_process_containment::PreparedContainmentBackend for CargoTestPrepared {
    fn kind(&self) -> temper_process_containment::ContainmentBackendKind {
        temper_process_containment::ContainmentBackendKind::LinuxSupervisor
    }

    fn root_identity(&self) -> temper_process_containment::ContainmentRootIdentity {
        self.root.clone()
    }

    fn spawn_precontained(
        self: Box<Self>,
        command: temper_process_containment::ContainmentCommand,
    ) -> io::Result<temper_process_containment::BackendSpawn> {
        let mut command = command.into_std_command();
        temper_process_containment::configure_command(&mut command);
        let child = command.spawn()?;
        let pid = child.id();
        Ok(temper_process_containment::BackendSpawn::new(
            child,
            Box::new(CargoTestKernel {
                pid,
                root: self.root,
                reaped: None,
            }),
        ))
    }
}

#[cfg(all(target_os = "linux", debug_assertions))]
struct CargoTestKernel {
    pid: u32,
    root: temper_process_containment::ContainmentRootIdentity,
    reaped: Option<Option<i32>>,
}

#[cfg(all(target_os = "linux", debug_assertions))]
impl CargoTestKernel {
    fn identity(&self) -> temper_process_containment::ProcessIdentity {
        temper_process_containment::ProcessIdentity::new(
            self.pid,
            std::process::id(),
            self.pid,
            self.pid,
            0,
            PathBuf::from("cargo-test-payload"),
        )
    }

    fn group_exists(&self) -> io::Result<bool> {
        if std::fs::read_to_string(format!("/proc/{}/stat", self.pid))
            .ok()
            .and_then(|stat| stat.rsplit_once(") ").map(|(_, fields)| fields.to_string()))
            .and_then(|fields| fields.split_whitespace().next().map(str::to_owned))
            .is_some_and(|state| state == "Z")
        {
            return Ok(false);
        }
        let group = format!("-{}", self.pid);
        Ok(std::process::Command::new("/bin/kill")
            .args(["-0", "--", group.as_str()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?
            .success())
    }
}

#[cfg(all(target_os = "linux", debug_assertions))]
impl temper_process_containment::ContainmentKernel for CargoTestKernel {
    fn backend_kind(&self) -> temper_process_containment::ContainmentBackendKind {
        temper_process_containment::ContainmentBackendKind::LinuxSupervisor
    }

    fn root_identity(&self) -> temper_process_containment::ContainmentRootIdentity {
        self.root.clone()
    }

    fn discover_members(&mut self) -> io::Result<temper_process_containment::MemberDiscovery> {
        if self.group_exists()? {
            Ok(temper_process_containment::MemberDiscovery::new(
                vec![self.identity()],
                0,
            ))
        } else {
            Ok(temper_process_containment::MemberDiscovery::empty())
        }
    }

    fn signal_members(
        &mut self,
        signal: temper_process_containment::ContainmentSignal,
    ) -> io::Result<temper_process_containment::SignalBatch> {
        let identity = self.identity();
        let group = format!("-{}", self.pid);
        let signal_name = match signal {
            temper_process_containment::ContainmentSignal::Term => "TERM",
            temper_process_containment::ContainmentSignal::Kill => "KILL",
        };
        let result = std::process::Command::new("/bin/kill")
            .args(["-s", signal_name, "--", group.as_str()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        let attempt = if result.success() {
            temper_process_containment::SignalAttempt::succeeded(identity, signal)
        } else if !self.group_exists()? {
            temper_process_containment::SignalAttempt::process_gone(identity, signal)
        } else {
            temper_process_containment::SignalAttempt::failed(
                identity,
                signal,
                format!("/bin/kill exited with {result}"),
            )
        };
        Ok(temper_process_containment::SignalBatch::new(
            vec![attempt],
            0,
        ))
    }

    fn reap_direct_child(
        &mut self,
        child: &mut std::process::Child,
    ) -> io::Result<temper_process_containment::DirectChildReap> {
        if let Some(exit_code) = self.reaped {
            return Ok(temper_process_containment::DirectChildReap::AlreadyReaped {
                pid: self.pid,
                exit_code,
            });
        }
        match child.try_wait()? {
            Some(status) => {
                let exit_code = status.code();
                self.reaped = Some(exit_code);
                Ok(temper_process_containment::DirectChildReap::Reaped {
                    pid: self.pid,
                    exit_code,
                })
            }
            None => Ok(temper_process_containment::DirectChildReap::Pending { pid: self.pid }),
        }
    }

    fn verify_recursive_empty(
        &mut self,
    ) -> io::Result<temper_process_containment::RecursiveEmptyProof> {
        if self.group_exists()? {
            Ok(temper_process_containment::RecursiveEmptyProof::not_empty(
                vec![self.identity()],
                0,
            ))
        } else {
            Ok(temper_process_containment::RecursiveEmptyProof::proven(1))
        }
    }
}

#[cfg(windows)]
fn production_backend_factory() -> Arc<dyn ContainmentBackendFactory> {
    Arc::new(temper_process_containment::WindowsJobBackendFactory)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn production_backend_factory() -> Arc<dyn ContainmentBackendFactory> {
    Arc::new(temper_process_containment::UnsupportedPlatformBackendFactory)
}
