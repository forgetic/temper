use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use super::*;

pub(super) fn proc_identity(pid: u32) -> io::Result<ProcessIdentity> {
    let stat_path = format!("/proc/{pid}/stat");
    let stat = fs::read_to_string(stat_path).map_err(normalize_proc_error)?;
    let close = stat.rfind(')').ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "malformed /proc stat command")
    })?;
    let fields: Vec<_> = stat[close + 1..].split_whitespace().collect();
    if fields.len() <= 19 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed /proc stat fields",
        ));
    }
    let parse = |index: usize, name: &str| -> io::Result<u64> {
        fields[index].parse().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid {name} `{}` in /proc stat for pid {pid}: {error}",
                    fields[index]
                ),
            )
        })
    };
    let parse_process_id = |index: usize, name: &str| -> io::Result<u32> {
        let value = fields[index].parse::<i64>().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid {name} `{}` in /proc stat for pid {pid}: {error}",
                    fields[index]
                ),
            )
        })?;
        // proc_pid_stat(5) uses signed ancestry fields and can expose -1 for a
        // task in release. Preserve the exact PID/start identity and represent
        // that unavailable relationship with ProcessIdentity's zero sentinel.
        if value == -1 {
            return Ok(0);
        }
        u32::try_from(value).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{name} `{value}` is outside the process-id range"),
            )
        })
    };
    let ppid = parse_process_id(1, "ppid")?;
    let pgrp = parse_process_id(2, "process group")?;
    let session = parse_process_id(3, "session")?;
    let start_time = parse(19, "start time")?;
    let executable = fs::read_link(format!("/proc/{pid}/exe")).map_err(normalize_proc_error)?;
    Ok(ProcessIdentity::new(
        pid, ppid, pgrp, session, start_time, executable,
    ))
}

pub(super) fn normalize_proc_error(error: io::Error) -> io::Error {
    if error.raw_os_error() == Some(libc::ENOENT) {
        io::Error::new(io::ErrorKind::NotFound, error)
    } else {
        error
    }
}

pub(super) fn pidfd_open(pid: u32) -> io::Result<OwnedFd> {
    // SAFETY: pidfd_open accepts a numeric PID and zero flags and returns a new
    // descriptor owned by the caller.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd == -1 {
        Err(io::Error::last_os_error())
    } else {
        let fd = i32::try_from(fd).map_err(|_| io::Error::other("pidfd exceeds RawFd"))?;
        // SAFETY: successful pidfd_open returned this newly owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

pub(super) trait EmergencyProcess: Send {
    fn signal(&self, signal: ContainmentSignal) -> io::Result<()>;
}

struct PinnedEmergencyProcess(OwnedFd);

impl EmergencyProcess for PinnedEmergencyProcess {
    fn signal(&self, signal: ContainmentSignal) -> io::Result<()> {
        let native_signal = match signal {
            ContainmentSignal::Term => libc::SIGTERM,
            ContainmentSignal::Kill => libc::SIGKILL,
        };
        // SAFETY: this owner pins the process with an owned pidfd and sends no
        // pointers or diagnostic identity data.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.0.as_raw_fd(),
                native_signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

pub(super) fn open_emergency_process(pid: u32) -> io::Result<Box<dyn EmergencyProcess>> {
    pidfd_open(pid)
        .map(|pidfd| Box::new(PinnedEmergencyProcess(pidfd)) as Box<dyn EmergencyProcess>)
}

pub(super) unsafe fn write_all_fd(fd: RawFd, bytes: &[u8]) -> io::Result<()> {
    let mut written = 0;
    while written < bytes.len() {
        // SAFETY: the caller guarantees that fd is an open writable descriptor;
        // the byte slice remains valid for the duration of the syscall.
        let result =
            unsafe { libc::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written) };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if result == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "cgroup.procs write returned zero",
            ));
        }
        written += usize::try_from(result).map_err(|_| io::Error::other("negative write"))?;
    }
    Ok(())
}
