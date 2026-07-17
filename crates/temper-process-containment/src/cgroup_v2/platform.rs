use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use super::containment::PreparedControls;
use super::*;

pub(super) trait CgroupFileSystem: Send + Sync {
    fn exists(&self, path: &Path) -> bool;
    fn create_cgroup(&self, path: &Path) -> io::Result<()>;
    fn open_directory(&self, path: &Path) -> io::Result<File>;
    fn open_read(&self, path: &Path) -> io::Result<File>;
    fn open_write(&self, path: &Path) -> io::Result<File>;
    fn open_read_write(&self, path: &Path) -> io::Result<File>;
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn read_events(&self, path: &Path, preopened: &mut File) -> io::Result<String>;
    fn child_directories(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
    fn remove_cgroup(&self, path: &Path) -> io::Result<()>;
    fn write_cgroup_kill(&self, root: &Path, control: Option<&mut File>) -> io::Result<()>;
}

#[derive(Debug)]
pub(super) struct RealCgroupFileSystem;

impl CgroupFileSystem for RealCgroupFileSystem {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn create_cgroup(&self, path: &Path) -> io::Result<()> {
        fs::create_dir(path)
    }

    fn open_directory(&self, path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)
    }

    fn open_read(&self, path: &Path) -> io::Result<File> {
        OpenOptions::new().read(true).open(path)
    }

    fn open_write(&self, path: &Path) -> io::Result<File> {
        OpenOptions::new().write(true).open(path)
    }

    fn open_read_write(&self, path: &Path) -> io::Result<File> {
        OpenOptions::new().read(true).write(true).open(path)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        fs::read_to_string(path)
    }

    fn read_events(&self, _path: &Path, preopened: &mut File) -> io::Result<String> {
        read_preopened(preopened)
    }

    fn child_directories(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut directories = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() && !file_type.is_symlink() {
                directories.push(entry.path());
            }
        }
        Ok(directories)
    }

    fn remove_cgroup(&self, path: &Path) -> io::Result<()> {
        fs::remove_dir(path)
    }

    fn write_cgroup_kill(&self, _root: &Path, control: Option<&mut File>) -> io::Result<()> {
        let control =
            control.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cgroup.kill"))?;
        control.seek(SeekFrom::Start(0))?;
        control.write_all(b"1")
    }
}

pub(super) trait LinuxProcessApi: Send + Sync {
    fn pidfd_supported(&self) -> bool;
    fn identity(&self, pid: u32) -> io::Result<ProcessIdentity>;
    fn signal(
        &self,
        expected: &ProcessIdentity,
        signal: ContainmentSignal,
    ) -> io::Result<SignalAttemptOutcome>;
}

#[derive(Debug)]
pub(super) struct RealLinuxProcessApi;

impl LinuxProcessApi for RealLinuxProcessApi {
    fn pidfd_supported(&self) -> bool {
        let Ok(pidfd) = pidfd_open(std::process::id()) else {
            return false;
        };
        // Probe the send operation too: older kernels may expose pidfd_open
        // without implementing pidfd_send_signal. Signal zero performs only
        // permission/existence checking and cannot affect this process.
        // SAFETY: `pidfd` is owned and valid; null siginfo and zero flags are
        // the documented pidfd_send_signal contract.
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                pidfd.as_raw_fd(),
                0,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            ) == 0
        }
    }

    fn identity(&self, pid: u32) -> io::Result<ProcessIdentity> {
        proc_identity(pid)
    }

    fn signal(
        &self,
        expected: &ProcessIdentity,
        signal: ContainmentSignal,
    ) -> io::Result<SignalAttemptOutcome> {
        let pidfd = match pidfd_open(expected.pid()) {
            Ok(pidfd) => pidfd,
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {
                return Ok(SignalAttemptOutcome::ProcessGone);
            }
            Err(error) => return Ok(SignalAttemptOutcome::Failed(error.to_string())),
        };
        match proc_identity(expected.pid()) {
            Ok(current) if current.start_time_identity() != expected.start_time_identity() => {
                return Ok(SignalAttemptOutcome::PidReused);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SignalAttemptOutcome::ProcessGone);
            }
            Err(error) => return Err(error),
        }
        let native_signal = match signal {
            ContainmentSignal::Term => libc::SIGTERM,
            ContainmentSignal::Kill => libc::SIGKILL,
        };
        // SAFETY: the pidfd is owned and valid for this syscall; null siginfo
        // and zero flags are the documented pidfd_send_signal contract.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                pidfd.as_raw_fd(),
                native_signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result == 0 {
            Ok(SignalAttemptOutcome::Succeeded)
        } else {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(SignalAttemptOutcome::ProcessGone)
            } else {
                Ok(SignalAttemptOutcome::Failed(error.to_string()))
            }
        }
    }
}

pub(super) fn probe_system(
    config: &CgroupV2FactoryConfig,
    fs: &dyn CgroupFileSystem,
    processes: &dyn LinuxProcessApi,
) -> CgroupV2Capability {
    let pidfd = processes.pidfd_supported();
    let mount_layout = find_unified_mount();
    let inherited = inherited_scope_path(fs);
    match (mount_layout, inherited) {
        (Ok((mount, _current)), Some(inherited)) => {
            probe_delegated(config, fs, Some(mount), inherited, pidfd)
        }
        (Ok((mount, current)), None) => probe_delegated(config, fs, Some(mount), current, pidfd),
        (Err(error), Some(inherited)) => {
            // The descriptor may be a valid nested cgroup, but selection must
            // still fail closed when the unified mount itself cannot be
            // identified and reported.
            let mut capability = probe_delegated(config, fs, None, inherited, pidfd);
            if capability.diagnostic.is_none() {
                capability.diagnostic = Some(error.to_string());
            }
            capability
        }
        (Err(error), None) => CgroupV2Capability::unavailable(error.to_string(), pidfd),
    }
}

pub(super) fn probe_delegated(
    config: &CgroupV2FactoryConfig,
    fs: &dyn CgroupFileSystem,
    mount: Option<PathBuf>,
    delegated: PathBuf,
    pidfd: bool,
) -> CgroupV2Capability {
    let delegation = fs.exists(&delegated.join("cgroup.controllers"))
        && fs.open_write(&delegated.join("cgroup.procs")).is_ok();
    let dedicated = delegated.join(&config.subtree);
    let dedicated_existed = fs.exists(&dedicated);
    let mut diagnostic = None;
    let mut dedicated_ready = false;
    let mut probe_rollback_complete = true;
    let writable = if !delegation {
        diagnostic = Some(format!(
            "{} is not a delegated writable cgroup-v2 subtree",
            delegated.display()
        ));
        false
    } else {
        match ensure_cgroup(fs, &dedicated) {
            Ok(()) => {
                dedicated_ready = true;
                let probe_nonce = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let probe =
                    dedicated.join(format!(".probe-{}-{probe_nonce:x}", std::process::id()));
                match fs.create_cgroup(&probe) {
                    Ok(()) => {
                        // Drop every probe descriptor before rmdir. This also
                        // makes a failed removal unambiguously a rollback
                        // failure rather than an artifact of our open files.
                        let controls = fs.open_read_write(&probe.join("cgroup.procs")).map(|_| ());
                        let events = fs.open_read(&probe.join("cgroup.events")).map(|_| ());
                        let remove = fs.remove_cgroup(&probe);
                        probe_rollback_complete = remove.is_ok();
                        match (controls, events, remove) {
                            (Ok(()), Ok(()), Ok(())) => true,
                            (controls, events, remove) => {
                                diagnostic = Some(format!(
                                    "delegation probe {} failed (procs={:?}, events={:?}, remove={:?})",
                                    probe.display(),
                                    controls.err(),
                                    events.err(),
                                    remove.err()
                                ));
                                false
                            }
                        }
                    }
                    Err(error) => {
                        // Real cgroup mkdir is atomic. The explicit check also
                        // supports injected filesystems that fail after mkdir;
                        // auto-selection is forbidden if that partial probe
                        // cannot be removed.
                        if error.kind() != io::ErrorKind::AlreadyExists && fs.exists(&probe) {
                            let rollback = fs.remove_cgroup(&probe);
                            probe_rollback_complete = rollback.is_ok();
                            diagnostic = Some(format!(
                                "cannot create delegated probe {}: {error}; rollback={rollback:?}",
                                probe.display()
                            ));
                        } else {
                            diagnostic = Some(format!(
                                "cannot create delegated probe {}: {error}",
                                probe.display()
                            ));
                        }
                        false
                    }
                }
            }
            Err(error) => {
                if !dedicated_existed && fs.exists(&dedicated) {
                    let rollback = fs.remove_cgroup(&dedicated);
                    probe_rollback_complete = rollback.is_ok();
                    diagnostic = Some(format!(
                        "cannot create Temper cgroup subtree: {error}; rollback={rollback:?}"
                    ));
                } else {
                    diagnostic = Some(format!("cannot create Temper cgroup subtree: {error}"));
                }
                false
            }
        }
    };
    let kill = writable && fs.exists(&dedicated.join("cgroup.kill"));
    if !pidfd && diagnostic.is_none() {
        diagnostic = Some("pidfd_open/pidfd_send_signal are unavailable".to_owned());
    }
    CgroupV2Capability {
        unified_mount: mount,
        delegated_subtree: Some(delegated),
        dedicated_subtree: dedicated_ready.then_some(dedicated),
        delegation,
        writable_subtree: writable,
        cgroup_kill: kill,
        pidfd,
        probe_rollback_complete,
        diagnostic,
    }
}

pub(super) fn inherited_scope_path(fs: &dyn CgroupFileSystem) -> Option<PathBuf> {
    let path = PathBuf::from(format!("/proc/self/fd/{INHERITED_CGROUP_SCOPE_FD}"));
    if !fs.exists(&path.join("cgroup.procs")) || !fs.exists(&path.join("cgroup.events")) {
        return None;
    }

    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `stat` points to writable storage and fstatfs does not retain it.
    let result = unsafe { libc::fstatfs(INHERITED_CGROUP_SCOPE_FD, stat.as_mut_ptr()) };
    if result == -1 {
        return None;
    }
    // SAFETY: successful fstatfs initialized the structure.
    let stat = unsafe { stat.assume_init() };
    (stat.f_type as u64 == libc::CGROUP2_SUPER_MAGIC as u64).then_some(path)
}

pub(super) fn find_unified_mount() -> io::Result<(PathBuf, PathBuf)> {
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    let self_cgroup = fs::read_to_string("/proc/self/cgroup")?;
    let current = self_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "no unified cgroup-v2 membership",
            )
        })?;

    for line in mountinfo.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        if after.split_whitespace().next() != Some("cgroup2") {
            continue;
        }
        let fields: Vec<_> = before.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        let mount_root = unescape_mountinfo(fields[3]);
        let mount_point = PathBuf::from(unescape_mountinfo(fields[4]));
        let current_path = Path::new(current);
        let relative = current_path.strip_prefix(&mount_root).map_err(|_| {
            io::Error::other(format!(
                "current cgroup {current} is outside mounted root {mount_root}"
            ))
        })?;
        return Ok((mount_point.clone(), mount_point.join(relative)));
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no cgroup2 mount in /proc/self/mountinfo",
    ))
}

pub(super) fn unescape_mountinfo(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

pub(super) fn unavailable_error(capability: &CgroupV2Capability) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "delegated cgroup v2 is unavailable (mount={:?}, delegation={}, writable={}, cgroup.kill={}, pidfd={}, probe_rollback_complete={}): {}",
            capability.unified_mount(),
            capability.delegation(),
            capability.writable_subtree(),
            capability.cgroup_kill(),
            capability.pidfd(),
            capability.probe_rollback_complete(),
            capability
                .diagnostic()
                .unwrap_or("capability requirements not met")
        ),
    )
}

pub(super) fn ensure_cgroup(fs: &dyn CgroupFileSystem, path: &Path) -> io::Result<()> {
    if fs.exists(path) {
        Ok(())
    } else {
        fs.create_cgroup(path)
    }
}

pub(super) fn rollback_created(fs: &dyn CgroupFileSystem, created: &[PathBuf]) -> io::Result<()> {
    for path in created.iter().rev() {
        fs.remove_cgroup(path)?;
    }
    Ok(())
}

pub(super) fn rollback_containment(
    fs: &dyn CgroupFileSystem,
    processes: &dyn LinuxProcessApi,
    controls: &mut PreparedControls,
) -> io::Result<()> {
    for _ in 0..ROLLBACK_RETRIES {
        match scavenge_one(fs, processes, &controls.path, 1) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(ROLLBACK_RETRY);
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        format!(
            "{} remained populated during rollback",
            controls.path.display()
        ),
    ))
}

pub(super) fn scavenge_one(
    fs: &dyn CgroupFileSystem,
    processes: &dyn LinuxProcessApi,
    path: &Path,
    retries: usize,
) -> io::Result<()> {
    for index in 0..retries.max(1) {
        let populated = parse_populated(&fs.read_to_string(&path.join("cgroup.events"))?)?;
        let members = enumerate_members(fs, processes, path)?;
        if !populated && members.is_empty() {
            let directories = descendant_directories(fs, path)?;
            for directory in directories {
                fs.remove_cgroup(&directory)?;
            }
            return Ok(());
        }

        // A successful signal is only an attempt. Re-enumerate and retry the
        // complete nested tree until events and membership independently agree
        // that it is empty.
        let kill_path = path.join("cgroup.kill");
        if fs.exists(&kill_path) {
            let mut kill = fs.open_write(&kill_path)?;
            fs.write_cgroup_kill(path, Some(&mut kill))?;
        } else {
            for identity in members {
                let _ = processes.signal(&identity, ContainmentSignal::Kill)?;
            }
        }
        if index + 1 < retries.max(1) {
            std::thread::sleep(ROLLBACK_RETRY);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        format!("{} remains populated", path.display()),
    ))
}

pub(super) fn enumerate_members(
    fs: &dyn CgroupFileSystem,
    processes: &dyn LinuxProcessApi,
    root: &Path,
) -> io::Result<Vec<ProcessIdentity>> {
    let mut seen = HashSet::new();
    let mut identities = Vec::new();
    for directory in descendant_directories(fs, root)? {
        for line in fs.read_to_string(&directory.join("cgroup.procs"))?.lines() {
            let pid = parse_pid(line)?;
            if pid == 0 || !seen.insert(pid) {
                continue;
            }
            match processes.identity(pid) {
                Ok(identity) => identities.push(identity),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(identities)
}

pub(super) fn descendant_directories(
    fs: &dyn CgroupFileSystem,
    root: &Path,
) -> io::Result<Vec<PathBuf>> {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut directories = Vec::new();
    while let Some((path, depth)) = pending.pop() {
        for child in fs.child_directories(&path)? {
            if !child.starts_with(root) {
                return Err(io::Error::other(format!(
                    "cgroup traversal escaped {} through {}",
                    root.display(),
                    child.display()
                )));
            }
            pending.push((child, depth.saturating_add(1)));
        }
        directories.push((path, depth));
    }
    directories.sort_by(|(left_path, left_depth), (right_path, right_depth)| {
        right_depth
            .cmp(left_depth)
            .then_with(|| left_path.cmp(right_path))
    });
    Ok(directories.into_iter().map(|(path, _)| path).collect())
}

pub(super) fn parse_populated(events: &str) -> io::Result<bool> {
    let value = events.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next() == Some("populated"))
            .then(|| fields.next())
            .flatten()
    });
    match value {
        Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(value) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid cgroup.events populated value {value:?}"),
        )),
        None => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cgroup.events has no populated field",
        )),
    }
}

pub(super) fn parse_pid(line: &str) -> io::Result<u32> {
    line.trim().parse().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid PID in cgroup.procs: {error}"),
        )
    })
}

pub(super) fn bounded_diagnostic(mut diagnostic: String) -> String {
    if diagnostic.len() <= MAX_SCAVENGE_DIAGNOSTIC_BYTES {
        return diagnostic;
    }
    let mut end = MAX_SCAVENGE_DIAGNOSTIC_BYTES;
    while !diagnostic.is_char_boundary(end) {
        end -= 1;
    }
    diagnostic.truncate(end);
    diagnostic
}

pub(super) fn read_preopened(file: &mut File) -> io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut value = String::new();
    file.read_to_string(&mut value)?;
    Ok(value)
}

pub(super) fn scope_component(scope: &ContainmentScope) -> io::Result<String> {
    let component = match scope {
        ContainmentScope::Job => "job".to_owned(),
        ContainmentScope::Tool => "tool".to_owned(),
        ContainmentScope::Agent => "agent".to_owned(),
        ContainmentScope::McpServer => "mcp-server".to_owned(),
        ContainmentScope::WorkerCommand => "worker-command".to_owned(),
        ContainmentScope::PrePush => "pre-push".to_owned(),
        ContainmentScope::Custom(value) => encode_component(value, "scope")?,
    };
    Ok(component)
}

pub(super) fn encode_component(value: &str, label: &str) -> io::Result<String> {
    if value.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("cgroup {label} must not be empty"),
        ));
    }
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "_{byte:02x}");
        }
    }
    if encoded.len() > 160 {
        let hash = value.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
        encoded.truncate(140);
        encoded.push('-');
        encoded.push_str(&format!("{hash:016x}"));
    }
    Ok(encoded)
}
