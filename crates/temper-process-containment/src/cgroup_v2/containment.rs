use std::collections::HashSet;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;

use super::*;

pub(super) struct PreparedControls {
    pub(super) path: PathBuf,
    pub(super) directory: File,
    pub(super) procs: File,
    pub(super) events: File,
    pub(super) kill: Option<File>,
}

impl PreparedControls {
    pub(super) fn open(fs: &dyn CgroupFileSystem, path: PathBuf) -> io::Result<Self> {
        let directory = fs.open_directory(&path)?;
        let procs = fs.open_read_write(&path.join("cgroup.procs"))?;
        let events = fs.open_read(&path.join("cgroup.events"))?;
        let kill_path = path.join("cgroup.kill");
        let kill = if fs.exists(&kill_path) {
            Some(fs.open_write(&kill_path)?)
        } else {
            None
        };
        Ok(Self {
            path,
            directory,
            procs,
            events,
            kill,
        })
    }
}

pub(super) struct CgroupV2PreparedContainment {
    pub(super) controls: Option<PreparedControls>,
    pub(super) root: ContainmentRootIdentity,
    pub(super) fs: Arc<dyn CgroupFileSystem>,
    pub(super) processes: Arc<dyn LinuxProcessApi>,
}

impl PreparedContainmentBackend for CgroupV2PreparedContainment {
    fn kind(&self) -> ContainmentBackendKind {
        ContainmentBackendKind::LinuxCgroupV2
    }

    fn root_identity(&self) -> ContainmentRootIdentity {
        self.root.clone()
    }

    fn spawn_precontained(
        mut self: Box<Self>,
        command: ContainmentCommand,
    ) -> io::Result<BackendSpawn> {
        let mut controls = self
            .controls
            .take()
            .expect("prepared cgroup controls are consumed exactly once");
        let procs_fd = controls.procs.as_raw_fd();
        let directory_fd = controls.directory.as_raw_fd();
        let mut command = command.into_std_command();
        // SAFETY: only async-signal-safe write/dup2/fcntl operations run after
        // fork. All paths and descriptors were prepared in the parent.
        unsafe {
            command.pre_exec(move || {
                write_all_fd(procs_fd, b"0")?;
                if directory_fd != INHERITED_CGROUP_SCOPE_FD
                    && libc::dup2(directory_fd, INHERITED_CGROUP_SCOPE_FD) == -1
                {
                    return Err(io::Error::last_os_error());
                }
                let flags = libc::fcntl(INHERITED_CGROUP_SCOPE_FD, libc::F_GETFD);
                if flags == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::fcntl(
                    INHERITED_CGROUP_SCOPE_FD,
                    libc::F_SETFD,
                    flags & !libc::FD_CLOEXEC,
                ) == -1
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }

        match command.spawn() {
            Ok(child) => {
                let kernel = CgroupV2Containment {
                    root: self.root.clone(),
                    controls,
                    fs: Arc::clone(&self.fs),
                    processes: Arc::clone(&self.processes),
                    inspections: 0,
                    removed: false,
                    direct_child_reaped: None,
                };
                Ok(BackendSpawn::new(child, Box::new(kernel)))
            }
            Err(spawn_error) => {
                let rollback =
                    rollback_containment(self.fs.as_ref(), self.processes.as_ref(), &mut controls);
                match rollback {
                    Ok(()) => Err(spawn_error),
                    Err(rollback_error) => Err(io::Error::other(format!(
                        "spawn failed: {spawn_error}; cgroup rollback failed: {rollback_error}"
                    ))),
                }
            }
        }
    }
}

impl Drop for CgroupV2PreparedContainment {
    fn drop(&mut self) {
        if let Some(mut controls) = self.controls.take() {
            // No payload was spawned, so an empty proof and removal should be
            // immediate. Drop cannot report an error; a later startup
            // scavenging pass retains and diagnoses anything uninspectable.
            let _ = rollback_containment(self.fs.as_ref(), self.processes.as_ref(), &mut controls);
        }
    }
}

/// Kernel implementation for one prepared cgroup-v2 ownership boundary.
pub struct CgroupV2Containment {
    pub(super) root: ContainmentRootIdentity,
    pub(super) controls: PreparedControls,
    pub(super) fs: Arc<dyn CgroupFileSystem>,
    pub(super) processes: Arc<dyn LinuxProcessApi>,
    pub(super) inspections: u64,
    pub(super) removed: bool,
    pub(super) direct_child_reaped: Option<(u32, Option<i32>)>,
}

impl CgroupV2Containment {
    fn discover_all(&mut self) -> io::Result<Vec<ProcessIdentity>> {
        if self.removed {
            return Ok(Vec::new());
        }
        let directories = descendant_directories(self.fs.as_ref(), &self.controls.path)?;
        let mut seen = HashSet::new();
        let mut members = Vec::new();
        for path in directories {
            let text = if path == self.controls.path {
                read_preopened(&mut self.controls.procs)?
            } else {
                self.fs.read_to_string(&path.join("cgroup.procs"))?
            };
            for line in text.lines() {
                let pid = parse_pid(line)?;
                // `0` is accepted by the fake filesystem as the pre-exec
                // membership marker; real cgroup.procs never reports it.
                if pid == 0 || !seen.insert(pid) {
                    continue;
                }
                match self.processes.identity(pid) {
                    Ok(identity) => members.push(identity),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        // Exit between cgroup.procs and /proc inspection is a
                        // normal race; the independent events/empty proof below
                        // decides whether cleanup can complete.
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        self.inspections = self.inspections.saturating_add(1);
        Ok(members)
    }

    fn root_populated(&mut self) -> io::Result<bool> {
        let events = self.fs.read_events(
            &self.controls.path.join("cgroup.events"),
            &mut self.controls.events,
        )?;
        parse_populated(&events)
    }

    fn remove_empty_tree(&mut self) -> io::Result<()> {
        let directories = descendant_directories(self.fs.as_ref(), &self.controls.path)?;
        for path in directories {
            self.fs.remove_cgroup(&path)?;
        }
        self.removed = true;
        Ok(())
    }
}

impl ContainmentKernel for CgroupV2Containment {
    fn backend_kind(&self) -> ContainmentBackendKind {
        ContainmentBackendKind::LinuxCgroupV2
    }

    fn root_identity(&self) -> ContainmentRootIdentity {
        self.root.clone()
    }

    fn discover_members(&mut self) -> io::Result<MemberDiscovery> {
        Ok(MemberDiscovery::new(self.discover_all()?, 0))
    }

    fn signal_members(&mut self, signal: ContainmentSignal) -> io::Result<SignalBatch> {
        let members = self.discover_all()?;
        if signal == ContainmentSignal::Kill && self.controls.kill.is_some() {
            self.fs
                .write_cgroup_kill(&self.controls.path, self.controls.kill.as_mut())?;
            return Ok(SignalBatch::new(
                members
                    .into_iter()
                    .map(|process| SignalAttempt::succeeded(process, signal))
                    .collect(),
                0,
            ));
        }

        let mut attempts = Vec::with_capacity(members.len());
        for process in members {
            let outcome = self.processes.signal(&process, signal)?;
            attempts.push(match outcome {
                SignalAttemptOutcome::Succeeded => SignalAttempt::succeeded(process, signal),
                SignalAttemptOutcome::ProcessGone => SignalAttempt::process_gone(process, signal),
                SignalAttemptOutcome::PidReused => SignalAttempt::pid_reused(process, signal),
                SignalAttemptOutcome::Failed(error) => {
                    SignalAttempt::failed(process, signal, error)
                }
            });
        }
        Ok(SignalBatch::new(attempts, 0))
    }

    fn reap_direct_child(&mut self, child: &mut Child) -> io::Result<DirectChildReap> {
        if let Some((pid, exit_code)) = self.direct_child_reaped {
            return Ok(DirectChildReap::AlreadyReaped { pid, exit_code });
        }
        let pid = child.id();
        match child.try_wait()? {
            Some(status) => {
                let exit_code = status.code();
                self.direct_child_reaped = Some((pid, exit_code));
                Ok(DirectChildReap::Reaped { pid, exit_code })
            }
            None => Ok(DirectChildReap::Pending { pid }),
        }
    }

    fn verify_recursive_empty(&mut self) -> io::Result<RecursiveEmptyProof> {
        if self.removed {
            return Ok(RecursiveEmptyProof::proven(self.inspections));
        }
        if self.root_populated()? {
            let survivors = self.discover_all()?;
            let omitted = usize::from(survivors.is_empty());
            return Ok(RecursiveEmptyProof::not_empty(survivors, omitted));
        }
        let survivors = self.discover_all()?;
        if !survivors.is_empty() {
            return Ok(RecursiveEmptyProof::not_empty(survivors, 0));
        }
        self.remove_empty_tree()?;
        Ok(RecursiveEmptyProof::proven(self.inspections))
    }
}
