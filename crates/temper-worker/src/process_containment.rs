//! Worker composition for descendant-complete process containment.

use std::io;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use temper_process_containment::{
    ContainmentBackendFactory, ContainmentBackendPolicy, ContainmentCommand, ContainmentFactory,
    ContainmentIdentity, ContainmentScope, ContainmentSpec, PreparedContainment,
};

static NEXT_OWNER_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn prepare_with_observer(
    job: &str,
    attempt: &str,
    scope: ContainmentScope,
    owner: &str,
    observer: Option<Arc<dyn temper_process_containment::CleanupObserver>>,
) -> io::Result<PreparedContainment> {
    let factory = production_factory(job, attempt)?;
    let factory = match observer {
        Some(observer) => factory.with_observer(observer),
        None => factory,
    };
    let nonce = NEXT_OWNER_ID.fetch_add(1, Ordering::Relaxed);
    let identity =
        ContainmentIdentity::new(format!("{owner}-{nonce}"))?.with_owner_identifier(owner)?;
    let short_tool_grace = matches!(
        &scope,
        ContainmentScope::WorkerCommand | ContainmentScope::PrePush
    );
    let spec = ContainmentSpec::new(identity, scope);
    let spec = if short_tool_grace {
        spec.with_timing(
            std::time::Duration::from_millis(100),
            std::time::Duration::from_millis(10),
        )
    } else {
        spec
    };
    factory.prepare(spec)
}

pub(crate) fn production_factory(job: &str, attempt: &str) -> io::Result<ContainmentFactory> {
    #[cfg(target_os = "linux")]
    {
        use temper_process_containment::{CgroupV2BackendFactory, CgroupV2FactoryConfig};

        let config = CgroupV2FactoryConfig::new(job, attempt)?;
        let fallback: Arc<dyn ContainmentBackendFactory> =
            Arc::new(linux_supervisor_backend_factory());
        let backend: Arc<dyn ContainmentBackendFactory> =
            Arc::new(CgroupV2BackendFactory::system(config).with_fallback(fallback));
        return Ok(ContainmentFactory::new(
            ContainmentBackendPolicy::Auto,
            backend,
        ));
    }

    #[cfg(windows)]
    {
        let backend: Arc<dyn ContainmentBackendFactory> =
            Arc::new(temper_process_containment::WindowsJobBackendFactory);
        return Ok(ContainmentFactory::new(
            ContainmentBackendPolicy::RequireWindowsJob,
            backend,
        ));
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (job, attempt);
        let backend: Arc<dyn ContainmentBackendFactory> =
            Arc::new(temper_process_containment::UnsupportedPlatformBackendFactory);
        Ok(ContainmentFactory::new(
            ContainmentBackendPolicy::Auto,
            backend,
        ))
    }
}

/// Selects only genuine descendant-complete Linux backends.
///
/// A libtest executable cannot dispatch the supervisor's hidden mode from its
/// generated `main`, so Cargo test executables route the helper through the
/// package's compiled custom-harness fixture. That path runs the real
/// subreaper/pidfd supervisor and never infers recursive emptiness from
/// process-group membership.
#[cfg(target_os = "linux")]
fn linux_supervisor_backend_factory() -> temper_process_containment::LinuxSupervisorBackendFactory {
    compiled_test_supervisor_helper()
        .map(temper_process_containment::LinuxSupervisorBackendFactory::with_helper_executable)
        .unwrap_or_default()
}

/// Finds the already-built custom-harness helper only when this library is
/// running inside one of Cargo's test executables. Production binaries do not
/// live in `deps` and therefore keep the normal current-exe early-main protocol.
#[cfg(target_os = "linux")]
fn compiled_test_supervisor_helper() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let deps = executable.parent()?;
    if deps.file_name()? != "deps" {
        return None;
    }
    let helper = deps.parent()?.join(format!(
        "temper-worker-containment-fixture{}",
        std::env::consts::EXE_SUFFIX
    ));
    helper.is_file().then_some(helper)
}

/// Copies the stable spawn properties exposed by `std::process::Command` into
/// the move-only containment request. Callers supply stdio because Rust does
/// not expose configured `Stdio` handles through command introspection.
pub(crate) fn containment_command(
    command: &Command,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
) -> ContainmentCommand {
    let mut contained = ContainmentCommand::new(command.get_program());
    contained.args(command.get_args());
    if let Some(cwd) = command.get_current_dir() {
        contained.current_dir(cwd);
    }
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => {
                contained.env(key, value);
            }
            None => {
                contained.env_remove(key);
            }
        }
    }
    contained.stdin(stdin).stdout(stdout).stderr(stderr);
    contained
}
