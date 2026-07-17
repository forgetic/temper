//! Worker composition for descendant-complete process containment.

use std::io;
#[cfg(all(target_os = "linux", debug_assertions))]
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
    let identity = ContainmentIdentity::new(format!("{owner}-{nonce}"))?;
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
        #[cfg(debug_assertions)]
        if running_under_cargo_test() {
            let backend: Arc<dyn ContainmentBackendFactory> = Arc::new(HarnessBackendFactory);
            return Ok(ContainmentFactory::new(
                ContainmentBackendPolicy::Auto,
                backend,
            ));
        }
        use temper_process_containment::{
            CgroupV2BackendFactory, CgroupV2FactoryConfig, LinuxSupervisorBackendFactory,
        };

        let config = CgroupV2FactoryConfig::new(job, attempt)?;
        let fallback: Arc<dyn ContainmentBackendFactory> =
            Arc::new(LinuxSupervisorBackendFactory::new());
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

#[cfg(all(target_os = "linux", debug_assertions))]
fn running_under_cargo_test() -> bool {
    std::env::current_exe().ok().is_some_and(|executable| {
        executable
            .parent()
            .and_then(|parent| parent.file_name())
            .is_some_and(|name| name == "deps")
    })
}

/// Cargo's generated test harness cannot dispatch the production fallback's
/// hidden early-main protocol. Tests inject this process-group kernel only for
/// pre-existing fixtures that do not exercise escaped sessions; production
/// binaries can never select it. Descendant-complete fallback coverage remains
/// in temper-process-containment's custom harness.
#[cfg(all(target_os = "linux", debug_assertions))]
#[derive(Debug)]
struct HarnessBackendFactory;

#[cfg(all(target_os = "linux", debug_assertions))]
impl ContainmentBackendFactory for HarnessBackendFactory {
    fn prepare_backend(
        &self,
        _policy: ContainmentBackendPolicy,
        spec: &ContainmentSpec,
    ) -> io::Result<Box<dyn temper_process_containment::PreparedContainmentBackend>> {
        Ok(Box::new(HarnessPrepared {
            root: temper_process_containment::ContainmentRootIdentity::new(
                temper_process_containment::ContainmentBackendKind::LinuxSupervisor,
                format!("cargo-test:{}", spec.identity.as_str()),
            ),
        }))
    }
}

#[cfg(all(target_os = "linux", debug_assertions))]
struct HarnessPrepared {
    root: temper_process_containment::ContainmentRootIdentity,
}

#[cfg(all(target_os = "linux", debug_assertions))]
impl temper_process_containment::PreparedContainmentBackend for HarnessPrepared {
    fn kind(&self) -> temper_process_containment::ContainmentBackendKind {
        temper_process_containment::ContainmentBackendKind::LinuxSupervisor
    }

    fn root_identity(&self) -> temper_process_containment::ContainmentRootIdentity {
        self.root.clone()
    }

    fn spawn_precontained(
        self: Box<Self>,
        command: ContainmentCommand,
    ) -> io::Result<temper_process_containment::BackendSpawn> {
        let mut command = command.into_std_command();
        temper_process_containment::configure_command(&mut command);
        let child = command.spawn()?;
        let pid = child.id();
        Ok(temper_process_containment::BackendSpawn::new(
            child,
            Box::new(HarnessKernel {
                pid,
                root: self.root,
                reaped: None,
            }),
        ))
    }
}

#[cfg(all(target_os = "linux", debug_assertions))]
struct HarnessKernel {
    pid: u32,
    root: temper_process_containment::ContainmentRootIdentity,
    reaped: Option<Option<i32>>,
}

#[cfg(all(target_os = "linux", debug_assertions))]
impl HarnessKernel {
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
        Ok(Command::new("/bin/kill")
            .args(["-0", "--", group.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success())
    }
}

#[cfg(all(target_os = "linux", debug_assertions))]
impl temper_process_containment::ContainmentKernel for HarnessKernel {
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
        let result = Command::new("/bin/kill")
            .args(["-s", signal_name, "--", group.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
