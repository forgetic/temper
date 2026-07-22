//! Agent-scoped composition for descendant-complete process containment.
//!
//! One context is created at the agent process/in-process runner boundary and
//! cloned into every process-owning tool. Backend selection is therefore
//! instance scoped (and injectable in tests) rather than controlled through
//! process-global environment variables.

#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use temper_process_containment::{
    ContainmentBackendFactory, ContainmentBackendPolicy, ContainmentFactory, ContainmentIdentity,
    ContainmentScope, ContainmentSpec, EmergencyTerminationRegistry,
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

    /// Rebinds every process spawned from this context to the attempt-owned
    /// emergency authority. Existing backend/observer selection is preserved.
    pub fn with_emergency_registry(mut self, registry: EmergencyTerminationRegistry) -> Self {
        self.factory = self.factory.with_emergency_registry(registry);
        self
    }

    pub fn emergency_termination_registry(&self) -> EmergencyTerminationRegistry {
        self.factory.emergency_termination_registry()
    }

    /// Adds attempt-scoped cleanup delivery while preserving any observer
    /// installed by the caller's backend-injection seam.
    pub fn with_observer(
        mut self,
        observer: Arc<dyn temper_process_containment::CleanupObserver>,
    ) -> Self {
        self.factory = self.factory.with_additional_observer(observer);
        self
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
    use temper_process_containment::{CgroupV2BackendFactory, CgroupV2FactoryConfig};

    // Production and test callers share the same descendant-complete selector.
    // Tests that execute process-owning paths inject a helper-capable forced
    // supervisor through `AgentContainmentContext::new`; there is deliberately
    // no process-group backend that can counterfeit recursive emptiness.
    let process = std::process::id().to_string();
    let config = CgroupV2FactoryConfig::new(format!("agent-{process}"), "session")
        .expect("static agent cgroup components are valid");
    let supervisor: Arc<dyn ContainmentBackendFactory> =
        Arc::new(linux_supervisor_backend_factory());
    Arc::new(CgroupV2BackendFactory::system(config).with_fallback(supervisor))
}

/// Uses the package's compiled early-main helper when this library is running
/// in a generated Cargo test harness. The selected backend remains the real
/// subreaper/pidfd supervisor; only its helper executable changes.
#[cfg(target_os = "linux")]
fn linux_supervisor_backend_factory() -> temper_process_containment::LinuxSupervisorBackendFactory {
    compiled_test_supervisor_helper()
        .map(temper_process_containment::LinuxSupervisorBackendFactory::with_helper_executable)
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn compiled_test_supervisor_helper() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let deps = executable.parent()?;
    if deps.file_name()? != "deps" {
        return None;
    }
    let helper = deps.parent()?.join(format!(
        "temper-agent-containment-helper{}",
        std::env::consts::EXE_SUFFIX
    ));
    helper.is_file().then_some(helper)
}

#[cfg(windows)]
fn production_backend_factory() -> Arc<dyn ContainmentBackendFactory> {
    Arc::new(temper_process_containment::WindowsJobBackendFactory)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn production_backend_factory() -> Arc<dyn ContainmentBackendFactory> {
    Arc::new(temper_process_containment::UnsupportedPlatformBackendFactory)
}
