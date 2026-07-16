//! OS containment for one worker-owned subprocess tree.
//!
//! The public surface is safe. Platform-specific `unsafe` is kept in this tiny
//! leaf crate so the worker and the rest of the workspace retain their
//! `unsafe_code = "forbid"` policy.

use std::io;
use std::process::{Child, Command};
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};

/// The containment primitive active for a child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainmentKind {
    UnixProcessGroup,
    WindowsJobObject,
    DirectChildFallback,
}

/// Configures attributes that must be applied between fork and exec.
pub fn configure_command(command: &mut Command) {
    #[cfg(unix)]
    configure_unix(command, libc::SIGKILL);
    #[cfg(not(any(unix, windows)))]
    let _ = command;
}

/// Configures an isolated tool/server subtree whose leader can relay abrupt
/// owner loss to its process group. On Linux the leader receives SIGTERM; the
/// caller must install a TERM handler that hard-kills its own group. Windows
/// relies on the kill-on-close Job Object attached after spawn.
pub fn configure_descendant_command(command: &mut Command) {
    #[cfg(unix)]
    configure_unix(command, libc::SIGTERM);
    #[cfg(not(any(unix, windows)))]
    let _ = command;
}

#[cfg(unix)]
fn configure_unix(command: &mut Command, parent_death_signal: i32) {
    use std::os::unix::process::CommandExt as _;

    // A fresh process group gives the supervisor one signal target for the
    // agent, MCP servers, and commands spawned by tools.
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

/// A kill-on-owner-loss handle for the complete child tree.
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
    /// Attaches containment immediately after spawn.
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
            Ok(Self {
                kind: ContainmentKind::DirectChildFallback,
            })
        }
    }

    pub fn kind(&self) -> ContainmentKind {
        self.kind
    }

    /// Requests ordinary termination of every process in the containment.
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

    /// Unconditionally kills every process in the containment.
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
    // An already-empty group is the desired postcondition.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn unix_contract_is_an_isolated_process_group() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("exit 0");
        configure_command(&mut command);
        let mut child = command.spawn().expect("spawn contained child");
        let containment = ProcessContainment::attach(&child).expect("attach containment");
        assert_eq!(containment.kind(), ContainmentKind::UnixProcessGroup);
        child.wait().expect("reap child");
        containment
            .hard_kill(&mut child)
            .expect("empty group is a clean postcondition");
    }

    #[test]
    #[cfg(windows)]
    fn windows_contract_uses_a_kill_on_close_job_object() {
        let mut child = Command::new("cmd")
            .args(["/C", "ping", "-n", "30", "127.0.0.1", ">NUL"])
            .spawn()
            .expect("spawn child");
        let containment = ProcessContainment::attach(&child).expect("attach job object");
        assert_eq!(containment.kind(), ContainmentKind::WindowsJobObject);
        drop(containment);
        child.wait().expect("kill-on-close and reap child");
    }
}
