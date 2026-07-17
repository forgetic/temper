use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::fd::RawFd;
use std::path::PathBuf;

use crate::ProcessIdentity;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MemberRevalidation {
    Current,
    Gone,
    PidReused,
    AncestryChanged,
}

pub(super) fn revalidate_member(
    process: &ProcessIdentity,
    current: &BTreeMap<u32, ProcStat>,
    descendants: &BTreeSet<u32>,
) -> MemberRevalidation {
    let Some(stat) = current.get(&process.pid()) else {
        return MemberRevalidation::Gone;
    };
    if stat.start_time != process.start_time_identity() {
        return MemberRevalidation::PidReused;
    }
    if !descendants.contains(&process.pid()) {
        return MemberRevalidation::AncestryChanged;
    }
    MemberRevalidation::Current
}

pub(super) struct TrackedMember {
    pub(super) identity: ProcessIdentity,
    pub(super) pidfd: PidFd,
}

pub(super) struct PidFd(RawFd);

impl PidFd {
    pub(super) fn open(pid: u32) -> io::Result<Self> {
        // SAFETY: pidfd_open takes integer arguments and returns a new owned fd.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(i32::try_from(fd).map_err(|_| {
                io::Error::other("pidfd does not fit a file descriptor")
            })?))
        }
    }

    pub(super) fn send_signal(&self, signal: i32) -> io::Result<()> {
        // SAFETY: this uses the owned pidfd, a valid signal number, no siginfo,
        // and the kernel-required zero flags.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.0,
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

impl Drop for PidFd {
    fn drop(&mut self) {
        // SAFETY: this is the unique owned pidfd.
        let _ = unsafe { libc::close(self.0) };
    }
}

#[derive(Clone)]
pub(super) struct ProcStat {
    pub(super) pid: u32,
    pub(super) ppid: u32,
    pub(super) process_group: u32,
    pub(super) session: u32,
    pub(super) start_time: u64,
    pub(super) executable: PathBuf,
}

impl ProcStat {
    pub(super) fn identity(&self) -> ProcessIdentity {
        ProcessIdentity::new(
            self.pid,
            self.ppid,
            self.process_group,
            self.session,
            self.start_time,
            self.executable.clone(),
        )
    }
}

pub(super) fn scan_proc() -> io::Result<BTreeMap<u32, ProcStat>> {
    let mut processes = BTreeMap::new();
    for entry in fs::read_dir("/proc")? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        match read_proc_stat(pid) {
            Ok(stat) => {
                processes.insert(pid, stat);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {}
            Err(error) => return Err(error),
        }
    }
    Ok(processes)
}

pub(super) fn read_proc_stat(pid: u32) -> io::Result<ProcStat> {
    let contents = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let close = contents
        .rfind(") ")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed /proc pid stat"))?;
    let fields: Vec<&str> = contents[close + 2..].split_whitespace().collect();
    if fields.len() <= 19 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "short /proc pid stat",
        ));
    }
    let parse = |index: usize, name: &str| -> io::Result<u64> {
        fields[index].parse().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid {name} in /proc pid stat"),
            )
        })
    };
    let executable = fs::read_link(format!("/proc/{pid}/exe"))
        .unwrap_or_else(|_| PathBuf::from(format!("[pid:{pid}]")));
    Ok(ProcStat {
        pid,
        ppid: u32::try_from(parse(1, "ppid")?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "ppid exceeds u32"))?,
        process_group: u32::try_from(parse(2, "process group")?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "process group exceeds u32"))?,
        session: u32::try_from(parse(3, "session")?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "session exceeds u32"))?,
        start_time: parse(19, "start time")?,
        executable,
    })
}

pub(super) fn descendants_of(root: u32, processes: &BTreeMap<u32, ProcStat>) -> BTreeSet<u32> {
    let mut descendants = BTreeSet::new();
    for &pid in processes.keys() {
        if pid == root {
            continue;
        }
        let mut cursor = pid;
        let mut visited = BTreeSet::new();
        while let Some(process) = processes.get(&cursor) {
            if !visited.insert(cursor) {
                break;
            }
            if process.ppid == root {
                descendants.insert(pid);
                break;
            }
            if process.ppid == 0 || process.ppid == cursor {
                break;
            }
            cursor = process.ppid;
        }
    }
    descendants
}

#[cfg(test)]
mod tests {
    use super::super::protocol::{read_process_line, write_process};
    use super::super::{LinuxAutoBackendFactory, LinuxSupervisorBackendFactory};
    use super::*;
    use crate::{
        ContainmentBackendFactory, ContainmentBackendKind, ContainmentBackendPolicy,
        ContainmentSpec,
    };

    #[test]
    fn proc_stat_parser_uses_start_time_identity() {
        let own = read_proc_stat(std::process::id()).expect("read own proc identity");
        assert_eq!(own.pid, std::process::id());
        assert!(own.start_time > 0);
        assert!(!own.executable.as_os_str().is_empty());
    }

    #[test]
    fn process_protocol_round_trip_preserves_identity() {
        let process = ProcessIdentity::new(12, 3, 4, 5, 99, "/tmp/a path/temper-agent");
        let mut bytes = Vec::new();
        write_process(&mut bytes, &process).expect("encode identity");
        let decoded = read_process_line(&mut bytes.as_slice()).expect("decode identity");
        assert_eq!(decoded, process);
    }

    #[test]
    fn auto_selector_uses_supervisor_only_after_cgroup_is_unavailable() {
        let spec = ContainmentSpec::new(
            crate::ContainmentIdentity::new("auto-fallback").expect("identity"),
            crate::ContainmentScope::Tool,
        );
        assert!(
            LinuxSupervisorBackendFactory::new()
                .prepare_backend(ContainmentBackendPolicy::Auto, &spec)
                .is_err(),
            "the supervisor alone must not claim Linux Auto selection"
        );
        let prepared = LinuxAutoBackendFactory::default()
            .prepare_backend(ContainmentBackendPolicy::Auto, &spec)
            .expect("Auto falls back when no delegated cgroup is installed");
        assert_eq!(prepared.kind(), ContainmentBackendKind::LinuxSupervisor);
    }

    #[test]
    fn pid_start_mismatch_is_detectable_before_signaling() {
        let own = read_proc_stat(std::process::id()).expect("read own proc identity");
        let mismatched = ProcessIdentity::new(
            own.pid,
            own.ppid,
            own.process_group,
            own.session,
            own.start_time.saturating_add(1),
            own.executable,
        );
        let current = scan_proc().expect("scan proc for identity revalidation");
        let descendants = BTreeSet::from([mismatched.pid()]);
        assert_eq!(
            revalidate_member(&mismatched, &current, &descendants),
            MemberRevalidation::PidReused
        );
    }
}
