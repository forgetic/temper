#[cfg(target_os = "linux")]
mod linux {
    use std::collections::BTreeSet;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitCode, Stdio};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use temper_process_containment::{
        CleanupTrigger, ContainmentBackendFactory, ContainmentBackendPolicy, ContainmentCommand,
        ContainmentFactory, ContainmentIdentity, ContainmentScope, ContainmentSpec,
        DirectChildReap, LinuxSupervisorBackendFactory, RecursiveEmptyProof,
        dispatch_linux_supervisor_helper,
    };

    const OWNER_FIXTURE: &str = "--owner-loss-fixture";

    pub fn main() -> ExitCode {
        if let Some(status) = dispatch_linux_supervisor_helper(std::env::args_os().skip(1)) {
            return status;
        }
        let arguments: Vec<_> = std::env::args_os().skip(1).collect();
        if arguments
            .first()
            .is_some_and(|value| value == OWNER_FIXTURE)
        {
            if let Err(error) = run_owner_fixture(&arguments[1..]) {
                eprintln!("Linux supervisor owner fixture failed: {error}");
                return ExitCode::FAILURE;
            }
            unreachable!("owner fixture exits without running destructors");
        }
        match run_contract_tests() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Linux supervisor contract test failed: {error}");
                ExitCode::FAILURE
            }
        }
    }

    fn run_contract_tests() -> io::Result<()> {
        concurrent_nested_sessions_are_reaped_once()?;
        explicit_cleanup_is_coordinated_once()?;
        owner_channel_loss_cleans_the_containment()?;
        Ok(())
    }

    fn concurrent_nested_sessions_are_reaped_once() -> io::Result<()> {
        let temporary = tempfile::tempdir()?;
        let script = write_payload_script(temporary.path())?;
        let mut containments = Vec::new();
        let mut identities = Vec::new();
        let mut root_values = BTreeSet::new();

        for index in 0..4 {
            let pid_file = temporary.path().join(format!("member-{index}.pid"));
            let cleanup_file = temporary.path().join(format!("cleanup-{index}.log"));
            let process = spawn_supervised(
                &format!("concurrent-{index}"),
                &script,
                &pid_file,
                &cleanup_file,
                23,
            )?;
            root_values.insert(process.root_identity().value().to_owned());
            let identity = wait_for_identity(&pid_file, Duration::from_secs(3))?;
            containments.push((process, cleanup_file));
            identities.push(identity);
        }
        if root_values.len() != containments.len() {
            return Err(io::Error::other(
                "concurrent supervisor containments did not receive unique roots",
            ));
        }

        for (process, cleanup_file) in containments {
            let status = process.wait_root()?;
            if status.code() != Some(23) {
                return Err(io::Error::other(format!(
                    "supervisor did not mirror payload exit 23: {status:?}"
                )));
            }
            // The detached session deliberately survives TERM. Observing its
            // one-shot TERM trap and its absence before the mirrored status is
            // accepted proves the helper did not exit with the payload leader.
            let report = process.cleanup(CleanupTrigger::NormalRootExit);
            match report.direct_child_reap() {
                DirectChildReap::Reaped {
                    exit_code: Some(23),
                    ..
                }
                | DirectChildReap::AlreadyReaped {
                    exit_code: Some(23),
                    ..
                } => {}
                other => {
                    return Err(io::Error::other(format!(
                        "unexpected exact helper reap status: {other:?}"
                    )));
                }
            }
            if !matches!(report.recursive_empty(), RecursiveEmptyProof::Proven { .. }) {
                return Err(io::Error::other("recursive emptiness was not proven"));
            }
            if report.term_attempts().is_empty() || report.kill_attempts().is_empty() {
                return Err(io::Error::other(
                    "automatic helper cleanup snapshots were not retained",
                ));
            }
            assert_cleanup_once(&cleanup_file)?;
        }

        for identity in identities {
            wait_until_gone(identity, Duration::from_secs(3))?;
        }
        Ok(())
    }

    fn explicit_cleanup_is_coordinated_once() -> io::Result<()> {
        let temporary = tempfile::tempdir()?;
        let script = write_payload_script(temporary.path())?;
        let pid_file = temporary.path().join("explicit.pid");
        let cleanup_file = temporary.path().join("explicit-cleanup.log");
        let process = spawn_supervised("explicit-cleanup", &script, &pid_file, &cleanup_file, 0)?;
        let identity = wait_for_identity(&pid_file, Duration::from_secs(3))?;
        let report = process.cleanup(CleanupTrigger::Cancellation);
        if report.term_attempts().is_empty() || report.kill_attempts().is_empty() {
            return Err(io::Error::other(
                "owner-driven cleanup did not retain TERM and KILL snapshots",
            ));
        }
        assert_cleanup_once(&cleanup_file)?;
        wait_until_gone(identity, Duration::from_secs(3))
    }

    fn owner_channel_loss_cleans_the_containment() -> io::Result<()> {
        let temporary = tempfile::tempdir()?;
        let script = write_payload_script(temporary.path())?;
        let pid_file = temporary.path().join("owner-loss.pid");
        let cleanup_file = temporary.path().join("owner-loss-cleanup.log");
        let identity_file = temporary.path().join("owner-loss-identity");
        let owner = std::env::current_exe()?;
        let status = Command::new(owner)
            .arg(OWNER_FIXTURE)
            .arg(&script)
            .arg(&pid_file)
            .arg(&cleanup_file)
            .arg(&identity_file)
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "abrupt owner fixture failed: {status:?}"
            )));
        }
        let identity = read_recorded_identity(&identity_file)?;
        wait_until_gone(identity, Duration::from_secs(5))?;
        wait_for_file(&cleanup_file, Duration::from_secs(5))?;
        assert_cleanup_once(&cleanup_file)
    }

    fn run_owner_fixture(arguments: &[std::ffi::OsString]) -> io::Result<()> {
        let [script, pid_file, cleanup_file, identity_file] = arguments else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "owner fixture requires script, pid, cleanup, and identity paths",
            ));
        };
        let process = spawn_supervised(
            "abrupt-owner",
            Path::new(script),
            Path::new(pid_file),
            Path::new(cleanup_file),
            0,
        )?;
        let identity = wait_for_identity(Path::new(pid_file), Duration::from_secs(3))?;
        fs::write(
            identity_file,
            format!("{} {}\n", identity.pid, identity.start_time),
        )?;
        // Keep the owning handle live until process teardown. `exit` bypasses
        // ContainedProcess::drop, so the private socket EOF is the only cleanup
        // trigger observed by the dedicated helper.
        std::mem::forget(process);
        std::process::exit(0);
    }

    fn spawn_supervised(
        identity: &str,
        script: &Path,
        pid_file: &Path,
        cleanup_file: &Path,
        exit_code: i32,
    ) -> io::Result<temper_process_containment::ContainedProcess> {
        let backend: Arc<dyn ContainmentBackendFactory> = Arc::new(
            LinuxSupervisorBackendFactory::with_helper_executable(std::env::current_exe()?),
        );
        let factory =
            ContainmentFactory::new(ContainmentBackendPolicy::ForceLinuxSupervisor, backend);
        let spec =
            ContainmentSpec::new(ContainmentIdentity::new(identity)?, ContainmentScope::Tool)
                .with_timing(Duration::from_millis(100), Duration::from_millis(10));
        let mut command = ContainmentCommand::new("/bin/sh");
        command
            .arg(script.as_os_str())
            .arg("leader")
            .arg(pid_file.as_os_str())
            .arg(cleanup_file.as_os_str())
            .arg(exit_code.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        factory.prepare(spec)?.spawn(command)
    }

    fn write_payload_script(directory: &Path) -> io::Result<PathBuf> {
        let path = directory.join("nested-session-fixture.sh");
        fs::write(
            &path,
            r#"#!/bin/sh
if [ "$1" = "descendant" ]; then
    echo "$$" > "$2"
    trap 'printf "cleaned\n" >> "$3"; trap "" TERM' TERM
    while :; do sleep 1; done
fi

# Exercise short-lived fork/exit and adopted-zombie races before creating a
# detached session that survives the payload leader.
i=0
while [ "$i" -lt 40 ]; do
    /bin/sh -c 'exit 0' &
    i=$((i + 1))
done
setsid /bin/sh "$0" descendant "$2" "$3" </dev/null >/dev/null 2>&1 &
limit=0
while [ ! -s "$2" ] && [ "$limit" -lt 100 ]; do
    sleep 0.01
    limit=$((limit + 1))
done
# Keep the leader present long enough for the owner test to capture the exact
# descendant start identity before automatic cleanup begins.
sleep 1
exit "$4"
"#,
        )?;
        Ok(path)
    }

    #[derive(Clone, Copy)]
    struct ProcessStartIdentity {
        pid: u32,
        start_time: u64,
    }

    fn wait_for_identity(path: &Path, timeout: Duration) -> io::Result<ProcessStartIdentity> {
        wait_for_file(path, timeout)?;
        let pid: u32 = fs::read_to_string(path)?.trim().parse().map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, format!("invalid pid: {error}"))
        })?;
        let start_time = proc_start_time(pid)?;
        Ok(ProcessStartIdentity { pid, start_time })
    }

    fn read_recorded_identity(path: &Path) -> io::Result<ProcessStartIdentity> {
        let value = fs::read_to_string(path)?;
        let mut fields = value.split_whitespace();
        let pid = fields
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing recorded pid"))?
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let start_time = fields
            .next()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing recorded start time")
            })?
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(ProcessStartIdentity { pid, start_time })
    }

    fn wait_for_file(path: &Path, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out waiting for {}", path.display()),
        ))
    }

    fn wait_until_gone(identity: ProcessStartIdentity, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match proc_start_time(identity.pid) {
                Ok(current) if current == identity.start_time => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(_) | Err(_) => return Ok(()),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "pid {} with start time {} survived supervisor cleanup",
                identity.pid, identity.start_time
            ),
        ))
    }

    fn proc_start_time(pid: u32) -> io::Result<u64> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let close = stat
            .rfind(") ")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed proc stat"))?;
        stat[close + 2..]
            .split_whitespace()
            .nth(19)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "short proc stat"))?
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn assert_cleanup_once(path: &Path) -> io::Result<()> {
        wait_for_file(path, Duration::from_secs(3))?;
        let count = fs::read_to_string(path)?
            .lines()
            .filter(|line| *line == "cleaned")
            .count();
        if count != 1 {
            return Err(io::Error::other(format!(
                "helper cleanup ran {count} times for {}",
                path.display()
            )));
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {}
