//! Temporary adapter for callers that have not migrated to prepared containment.
//!
//! This module preserves source compatibility only. Unix process groups are
//! **not descendant-complete**: a descendant can escape by creating another
//! process group or session. No [`crate::ContainmentBackendKind`] or
//! [`crate::ContainmentFactory`] selector can choose this adapter.

use std::io;
use std::process::{Child, Command};
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};

/// The primitive used by the temporary migration adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainmentKind {
    /// Non-descendant-complete compatibility behavior.
    UnixProcessGroup,
    WindowsJobObject,
    /// No descendant-complete primitive exists for this target. Legacy attach
    /// returns an explicit error rather than claiming direct-child containment.
    UnsupportedPlatform,
}

/// Configures the legacy process-group migration adapter before spawn.
///
/// This adapter is not descendant-complete. New code must use
/// [`crate::ContainmentFactory::prepare`] and [`crate::PreparedContainment::spawn`].
pub fn configure_command(command: &mut Command) {
    #[cfg(unix)]
    configure_unix(command, libc::SIGKILL);
    #[cfg(not(any(unix, windows)))]
    let _ = command;
}

/// Configures the legacy process-group migration adapter for a nested owner.
///
/// This adapter is not descendant-complete. It exists only while process
/// owners migrate to the prepared contract.
pub fn configure_descendant_command(command: &mut Command) {
    #[cfg(unix)]
    configure_unix(command, libc::SIGTERM);
    #[cfg(not(any(unix, windows)))]
    let _ = command;
}

#[cfg(unix)]
fn configure_unix(command: &mut Command, parent_death_signal: i32) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);

    #[cfg(target_os = "linux")]
    {
        // SAFETY: pre_exec is limited to async-signal-safe libc calls. The
        // parent identity check closes the fork/prctl race: if the worker died
        // before PR_SET_PDEATHSIG was installed, the child exits before exec.
        unsafe {
            let expected_parent = libc::getpid();
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, parent_death_signal) == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != expected_parent {
                    libc::_exit(127);
                }
                Ok(())
            });
        }
    }
}

/// Kill-on-owner-loss handle for the temporary migration adapter.
///
/// On Unix this owns only a process group and is therefore explicitly not
/// descendant-complete.
pub struct ProcessContainment {
    #[cfg(unix)]
    process_group: i32,
    #[cfg(unix)]
    armed: AtomicBool,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    kind: ContainmentKind,
}

// SAFETY: the Job Object HANDLE is uniquely owned and Win32 permits using and
// closing it from a different thread than the creator.
#[cfg(windows)]
unsafe impl Send for ProcessContainment {}

impl ProcessContainment {
    /// Legacy post-spawn attachment. This interval is why new production code
    /// must use the prepared API instead.
    pub fn attach(child: &Child) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let process_group = i32::try_from(child.id())
                .map_err(|_| io::Error::other("child pid does not fit platform pid type"))?;
            Ok(Self {
                process_group,
                armed: AtomicBool::new(true),
                kind: ContainmentKind::UnixProcessGroup,
            })
        }
        #[cfg(windows)]
        {
            attach_windows(child)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = child;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "process containment is unsupported on this platform",
            ))
        }
    }

    pub fn kind(&self) -> ContainmentKind {
        self.kind
    }

    /// Always false for this compatibility surface. Even the old Windows path
    /// retains a post-spawn assignment interval and is not the prepared Windows
    /// backend contract.
    pub const fn is_descendant_complete(&self) -> bool {
        false
    }

    /// Requests ordinary termination of the legacy containment target.
    pub fn terminate(&self, _child: &mut Child) -> io::Result<()> {
        #[cfg(unix)]
        {
            signal_group(self.process_group, libc::SIGTERM)
        }
        #[cfg(windows)]
        {
            terminate_windows(self.job, 143)
        }
        #[cfg(not(any(unix, windows)))]
        _child.kill()
    }

    /// Unconditionally kills the legacy containment target.
    pub fn hard_kill(&self, _child: &mut Child) -> io::Result<()> {
        #[cfg(unix)]
        {
            let result = signal_group(self.process_group, libc::SIGKILL);
            if result.is_ok() {
                self.armed.store(false, Ordering::Release);
            }
            result
        }
        #[cfg(windows)]
        {
            terminate_windows(self.job, 137)
        }
        #[cfg(not(any(unix, windows)))]
        _child.kill()
    }
}

#[cfg(unix)]
fn signal_group(process_group: i32, signal: i32) -> io::Result<()> {
    // SAFETY: a negative, positive pgid targets only the isolated group created
    // for this run. No pointers or borrowed memory cross the call.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(windows)]
fn attach_windows(child: &Child) -> io::Result<ProcessContainment> {
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle as _;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    // SAFETY: all pointers are either null per the Win32 contract or point to
    // initialized storage for the duration of the call. The returned owned
    // handle is closed by Drop.
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job == null_mut() {
            return Err(io::Error::last_os_error());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            let error = io::Error::last_os_error();
            let _ = windows_sys::Win32::Foundation::CloseHandle(job);
            return Err(error);
        }
        if AssignProcessToJobObject(job, child.as_raw_handle().cast()) == 0 {
            let error = io::Error::last_os_error();
            let _ = windows_sys::Win32::Foundation::CloseHandle(job);
            return Err(error);
        }
        Ok(ProcessContainment {
            job,
            kind: ContainmentKind::WindowsJobObject,
        })
    }
}

#[cfg(windows)]
fn terminate_windows(
    job: windows_sys::Win32::Foundation::HANDLE,
    exit_code: u32,
) -> io::Result<()> {
    // SAFETY: `job` is an owned, live Job Object handle until Drop.
    if unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(job, exit_code) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for ProcessContainment {
    fn drop(&mut self) {
        if self.armed.swap(false, Ordering::AcqRel) {
            let _ = signal_group(self.process_group, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessContainment {
    fn drop(&mut self) {
        // SAFETY: this is the unique owned handle. KILL_ON_JOB_CLOSE provides
        // the abrupt-worker-death guarantee when Windows closes it for us.
        let _ = unsafe { windows_sys::Win32::Foundation::CloseHandle(self.job) };
    }
}
