use std::ffi::OsString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::PathBuf;
use std::process::{Child, ExitCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::{
    BackendSpawn, ContainmentBackendFactory, ContainmentBackendKind, ContainmentBackendPolicy,
    ContainmentCommand, ContainmentKernel, ContainmentRootIdentity, ContainmentSignal,
    ContainmentSpec, DirectChildReap, MemberDiscovery, PreparedContainmentBackend,
    RecursiveEmptyProof, SignalBatch,
};

mod helper;
mod process;
mod protocol;

use process::PidFd;
use protocol::{ProtocolFrame, SupervisorClient};

pub(super) const HELPER_MODE: &str = "--temper-linux-supervisor-helper";
static NEXT_SUPERVISOR_ROOT: AtomicU64 = AtomicU64::new(0);

/// Prepared Linux fallback based on one dedicated child subreaper per
/// containment.
///
/// The helper is the current Temper executable by default. Tests and embedders
/// may provide another executable that performs the same early helper dispatch.
#[derive(Clone, Debug)]
pub struct LinuxSupervisorBackendFactory {
    helper_executable: Option<PathBuf>,
}

impl LinuxSupervisorBackendFactory {
    pub fn new() -> Self {
        Self {
            helper_executable: None,
        }
    }

    pub fn with_helper_executable(helper_executable: impl Into<PathBuf>) -> Self {
        Self {
            helper_executable: Some(helper_executable.into()),
        }
    }

    fn helper_executable(&self) -> io::Result<PathBuf> {
        match &self.helper_executable {
            Some(path) => Ok(path.clone()),
            None => std::env::current_exe(),
        }
    }
}

impl Default for LinuxSupervisorBackendFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl ContainmentBackendFactory for LinuxSupervisorBackendFactory {
    fn prepare_backend(
        &self,
        policy: ContainmentBackendPolicy,
        spec: &ContainmentSpec,
    ) -> io::Result<Box<dyn PreparedContainmentBackend>> {
        if policy != ContainmentBackendPolicy::ForceLinuxSupervisor {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "Linux supervisor requires ForceLinuxSupervisor; use the Linux Auto selector for {policy:?}"
                ),
            ));
        }
        ensure_pidfd_support()?;
        let helper_executable = self.helper_executable()?;
        let nonce = NEXT_SUPERVISOR_ROOT.fetch_add(1, Ordering::Relaxed);
        let root = ContainmentRootIdentity::new(
            ContainmentBackendKind::LinuxSupervisor,
            format!(
                "subreaper:{}:{}:{nonce}",
                std::process::id(),
                spec.identity.as_str()
            ),
        );
        Ok(Box::new(PreparedLinuxSupervisor {
            helper_executable,
            root,
            term_grace: spec.term_grace,
            inspection_retry: spec.inspection_retry,
        }))
    }
}

/// Linux Auto selection seam. A delegated-cgroup implementation is attempted
/// first when one is installed; only an explicit "unavailable on this host"
/// I/O classification falls back to the deterministic supervisor.
#[derive(Clone)]
pub struct LinuxAutoBackendFactory {
    delegated_cgroup: Option<Arc<dyn ContainmentBackendFactory>>,
    supervisor: LinuxSupervisorBackendFactory,
}

impl LinuxAutoBackendFactory {
    pub fn supervisor_only() -> Self {
        Self {
            delegated_cgroup: None,
            supervisor: LinuxSupervisorBackendFactory::new(),
        }
    }

    pub fn with_delegated_cgroup(
        delegated_cgroup: Arc<dyn ContainmentBackendFactory>,
        supervisor: LinuxSupervisorBackendFactory,
    ) -> Self {
        Self {
            delegated_cgroup: Some(delegated_cgroup),
            supervisor,
        }
    }
}

impl Default for LinuxAutoBackendFactory {
    fn default() -> Self {
        Self::supervisor_only()
    }
}

impl ContainmentBackendFactory for LinuxAutoBackendFactory {
    fn prepare_backend(
        &self,
        policy: ContainmentBackendPolicy,
        spec: &ContainmentSpec,
    ) -> io::Result<Box<dyn PreparedContainmentBackend>> {
        match policy {
            ContainmentBackendPolicy::ForceLinuxSupervisor => {
                self.supervisor.prepare_backend(policy, spec)
            }
            ContainmentBackendPolicy::RequireWindowsJob => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Windows Job Objects are unavailable on Linux",
            )),
            ContainmentBackendPolicy::RequireCgroupV2 => self
                .delegated_cgroup
                .as_ref()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::Unsupported,
                        "no delegated cgroup-v2 backend is installed",
                    )
                })?
                .prepare_backend(policy, spec),
            ContainmentBackendPolicy::Auto => {
                if let Some(cgroup) = &self.delegated_cgroup {
                    match cgroup.prepare_backend(policy, spec) {
                        Ok(prepared) => return Ok(prepared),
                        Err(error) if cgroup_is_unavailable(&error) => {}
                        Err(error) => return Err(error),
                    }
                }
                self.supervisor
                    .prepare_backend(ContainmentBackendPolicy::ForceLinuxSupervisor, spec)
            }
        }
    }
}

fn cgroup_is_unavailable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Unsupported
            | io::ErrorKind::PermissionDenied
            | io::ErrorKind::NotFound
            | io::ErrorKind::ReadOnlyFilesystem
    )
}

struct PreparedLinuxSupervisor {
    helper_executable: PathBuf,
    root: ContainmentRootIdentity,
    term_grace: Duration,
    inspection_retry: Duration,
}

impl PreparedContainmentBackend for PreparedLinuxSupervisor {
    fn kind(&self) -> ContainmentBackendKind {
        ContainmentBackendKind::LinuxSupervisor
    }

    fn root_identity(&self) -> ContainmentRootIdentity {
        self.root.clone()
    }

    fn spawn_precontained(
        self: Box<Self>,
        command: ContainmentCommand,
    ) -> io::Result<BackendSpawn> {
        let (owner_channel, helper_channel) = std::os::unix::net::UnixStream::pair()?;
        // Keep the private protocol away from the stdio descriptor range. This
        // matters for service launches whose parent deliberately closed one of
        // fd 0, 1, or 2 before preparing containment.
        let owner_channel = move_stream_above_stdio(owner_channel)?;
        let helper_channel = move_stream_above_stdio(helper_channel)?;
        let writer = owner_channel.try_clone()?;
        let helper_fd = helper_channel.as_raw_fd();
        let helper_arguments = vec![
            OsString::from(HELPER_MODE),
            OsString::from(helper_fd.to_string()),
            OsString::from(duration_millis(self.term_grace).to_string()),
            OsString::from(duration_millis(self.inspection_retry).to_string()),
        ];
        let mut helper = command
            .into_linux_supervisor_command(self.helper_executable.as_os_str(), helper_arguments);

        // SAFETY: this closure runs in the post-fork helper process. It only
        // changes the close-on-exec bit of the valid socket fd captured above;
        // no allocation or borrowed pointer crosses the call.
        unsafe {
            helper.pre_exec(move || helper::set_close_on_exec(helper_fd, false));
        }
        let mut child = helper.spawn()?;
        drop(helper_channel);

        let mut client = SupervisorClient::new(owner_channel, writer);
        let handshake = client.read_frame();
        match handshake {
            Ok(ProtocolFrame::Ready { payload_pid }) if payload_pid > 0 => {}
            Ok(ProtocolFrame::Error(message)) => {
                drop(client);
                let _ = child.wait();
                return Err(io::Error::other(format!(
                    "Linux supervisor failed before payload spawn: {message}"
                )));
            }
            Ok(frame) => {
                // Closing the owner endpoint is the helper's fail-closed
                // cleanup trigger. Never kill the helper itself: it may
                // already own a payload that only the subreaper can safely
                // attribute and reap.
                drop(client);
                let cleanup = child.wait();
                return Err(io::Error::other(format!(
                    "Linux supervisor sent invalid spawn handshake: {frame:?}; helper cleanup: {cleanup:?}"
                )));
            }
            Err(error) => {
                drop(client);
                let cleanup = child.wait();
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "Linux supervisor handshake failed: {error}; helper cleanup: {cleanup:?}"
                    ),
                ));
            }
        }

        let kernel = LinuxSupervisorKernel {
            root: self.root,
            client,
            inspections: 0,
            automatic_term_taken: false,
            automatic_kill_taken: false,
        };
        Ok(BackendSpawn::new(child, Box::new(kernel)))
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn ensure_pidfd_support() -> io::Result<()> {
    let pidfd = PidFd::open(std::process::id()).map_err(classify_pidfd_error)?;
    pidfd.send_signal(0).map_err(classify_pidfd_error)
}

fn classify_pidfd_error(error: io::Error) -> io::Error {
    if matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS) | Some(libc::EINVAL)
    ) {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("Linux supervisor requires pidfd_open and pidfd_send_signal: {error}"),
        )
    } else {
        error
    }
}

fn move_stream_above_stdio(
    stream: std::os::unix::net::UnixStream,
) -> io::Result<std::os::unix::net::UnixStream> {
    if stream.as_raw_fd() > libc::STDERR_FILENO {
        return Ok(stream);
    }
    // SAFETY: F_DUPFD_CLOEXEC duplicates the live socket into a newly owned fd
    // at or above 3. Ownership is transferred to UnixStream exactly once.
    let fd = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) })
}

struct LinuxSupervisorKernel {
    root: ContainmentRootIdentity,
    client: SupervisorClient,
    inspections: u64,
    automatic_term_taken: bool,
    automatic_kill_taken: bool,
}

impl ContainmentKernel for LinuxSupervisorKernel {
    fn backend_kind(&self) -> ContainmentBackendKind {
        ContainmentBackendKind::LinuxSupervisor
    }

    fn root_identity(&self) -> ContainmentRootIdentity {
        self.root.clone()
    }

    fn discover_members(&mut self) -> io::Result<MemberDiscovery> {
        self.inspections = self.inspections.saturating_add(1);
        match self.client.request(b'D')? {
            ProtocolFrame::Members { members, omitted } => {
                Ok(MemberDiscovery::new(members, omitted))
            }
            ProtocolFrame::Final { inspections, .. } => {
                self.inspections = self.inspections.max(inspections);
                Ok(MemberDiscovery::empty())
            }
            ProtocolFrame::Error(message) => Err(io::Error::other(message)),
            frame => Err(invalid_response("membership", &frame)),
        }
    }

    fn signal_members(&mut self, signal: ContainmentSignal) -> io::Result<SignalBatch> {
        let command = match signal {
            ContainmentSignal::Term => b'T',
            ContainmentSignal::Kill => b'K',
        };
        match self.client.request(command)? {
            ProtocolFrame::Attempts { attempts, omitted } => {
                Ok(SignalBatch::new(attempts, omitted))
            }
            ProtocolFrame::Final { inspections, .. } => {
                self.inspections = self.inspections.max(inspections);
                Ok(SignalBatch::new(Vec::new(), 0))
            }
            ProtocolFrame::Error(message) => Err(io::Error::other(message)),
            frame => Err(invalid_response("signal", &frame)),
        }
    }

    fn take_backend_signal_batch(&mut self, signal: ContainmentSignal) -> Option<SignalBatch> {
        let ProtocolFrame::Final {
            automatic_term,
            automatic_kill,
            ..
        } = self.client.terminal()?
        else {
            return None;
        };
        match signal {
            ContainmentSignal::Term if !self.automatic_term_taken => {
                self.automatic_term_taken = automatic_term.is_some();
                automatic_term.clone()
            }
            ContainmentSignal::Kill if !self.automatic_kill_taken => {
                self.automatic_kill_taken = automatic_kill.is_some();
                automatic_kill.clone()
            }
            _ => None,
        }
    }

    fn reap_direct_child(&mut self, child: &mut Child) -> io::Result<DirectChildReap> {
        let pid = child.id();
        match child.try_wait()? {
            Some(status) => {
                if let Some(ProtocolFrame::Final { payload_status, .. }) = self.client.terminal() {
                    if !helper_status_matches_payload(*payload_status, status) {
                        return Err(io::Error::other(format!(
                            "Linux supervisor exit status {status:?} did not mirror payload status {payload_status}"
                        )));
                    }
                }
                Ok(DirectChildReap::Reaped {
                    pid,
                    exit_code: status.code(),
                })
            }
            None => Ok(DirectChildReap::Pending { pid }),
        }
    }

    fn verify_recursive_empty(&mut self) -> io::Result<RecursiveEmptyProof> {
        match self.client.request(b'V')? {
            ProtocolFrame::Empty { inspections } | ProtocolFrame::Final { inspections, .. } => {
                self.inspections = self.inspections.max(inspections);
                Ok(RecursiveEmptyProof::proven(self.inspections))
            }
            ProtocolFrame::Members { members, omitted } => {
                Ok(RecursiveEmptyProof::not_empty(members, omitted))
            }
            ProtocolFrame::Error(message) => Err(io::Error::other(message)),
            frame => Err(invalid_response("empty verification", &frame)),
        }
    }
}

fn helper_status_matches_payload(
    payload_status: i32,
    helper_status: std::process::ExitStatus,
) -> bool {
    if payload_status >= 0 {
        helper_status.code() == Some(payload_status)
    } else {
        helper_status.signal() == Some(payload_status.saturating_neg())
    }
}

fn invalid_response(operation: &str, frame: &ProtocolFrame) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid Linux supervisor {operation} response: {frame:?}"),
    )
}

/// Dispatch the hidden Linux helper mode before logging, runtimes, or ordinary
/// CLI argument parsing starts. Returns `None` for every public invocation.
#[doc(hidden)]
pub fn dispatch_linux_supervisor_helper(
    arguments: impl IntoIterator<Item = OsString>,
) -> Option<ExitCode> {
    helper::dispatch(arguments)
}
