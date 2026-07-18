#[cfg(windows)]
mod windows {
    use std::fs;
    use std::io;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use std::path::Path;
    use std::process::{Command, ExitCode, Stdio};
    use std::ptr::null_mut;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use temper_process_containment::{
        CleanupTrigger, ContainmentBackendFactory, ContainmentBackendPolicy, ContainmentCommand,
        ContainmentFactory, ContainmentIdentity, ContainmentScope, ContainmentSpec,
        RecursiveEmptyProof, WindowsJobBackendFactory,
    };
    use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, WaitForSingleObject,
    };

    const MEMBERSHIP_FIXTURE: &str = "--membership-fixture";
    const NESTED_FIXTURE: &str = "--nested-fixture";
    const OWNER_FIXTURE: &str = "--owner-fixture";
    const LINGER_FIXTURE: &str = "--linger-fixture";
    const CONTRACT_TEST: &str = "windows_job_contract";
    const SYNCHRONIZE_PROCESS: u32 = 0x0010_0000;

    pub fn main() -> ExitCode {
        let arguments: Vec<_> = std::env::args_os().skip(1).collect();
        if arguments.iter().any(|argument| argument == "--list") {
            if !arguments.iter().any(|argument| argument == "--ignored") {
                println!("{CONTRACT_TEST}: test");
            }
            return ExitCode::SUCCESS;
        }
        let result = match arguments.first().and_then(|value| value.to_str()) {
            Some(MEMBERSHIP_FIXTURE) => membership_fixture(),
            Some(NESTED_FIXTURE) => one_path(&arguments[1..]).and_then(nested_fixture),
            Some(OWNER_FIXTURE) => one_path(&arguments[1..]).and_then(owner_fixture),
            Some(LINGER_FIXTURE) => {
                std::thread::sleep(Duration::from_secs(60));
                Ok(())
            }
            _ => run_contract_tests(),
        };
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Windows Job contract test failed: {error}");
                ExitCode::FAILURE
            }
        }
    }

    fn run_contract_tests() -> io::Result<()> {
        payload_is_assigned_before_its_first_instruction()?;
        nested_descendants_are_killed_and_empty_is_verified()?;
        kill_on_close_handles_abrupt_owner_loss()?;
        Ok(())
    }

    fn payload_is_assigned_before_its_first_instruction() -> io::Result<()> {
        let mut command = ContainmentCommand::new(std::env::current_exe()?);
        command
            .arg(MEMBERSHIP_FIXTURE)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let process = factory("pre-execution")?.spawn(command)?;
        let status = process.wait_root()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "payload did not observe pre-execution Job assignment: {status:?}"
            )));
        }
        let report = process.cleanup(CleanupTrigger::NormalRootExit);
        assert_empty(&report)
    }

    fn nested_descendants_are_killed_and_empty_is_verified() -> io::Result<()> {
        let temporary = tempfile::tempdir()?;
        let pid_file = temporary.path().join("nested.pid");
        let mut command = ContainmentCommand::new(std::env::current_exe()?);
        command
            .arg(NESTED_FIXTURE)
            .arg(&pid_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let process = factory("nested-descendants")?.spawn(command)?;
        let status = process.wait_root()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "nested fixture failed: {status:?}"
            )));
        }
        let pid = wait_for_pid(&pid_file, Duration::from_secs(5))?;
        let report = process.cleanup(CleanupTrigger::NormalRootExit);
        assert_empty(&report)?;
        if report.term_attempts().is_empty() || report.kill_attempts().is_empty() {
            return Err(io::Error::other(
                "nested Job cleanup omitted TERM/KILL contract evidence",
            ));
        }
        wait_until_process_gone(pid, Duration::from_secs(5))
    }

    fn kill_on_close_handles_abrupt_owner_loss() -> io::Result<()> {
        let temporary = tempfile::tempdir()?;
        let pid_file = temporary.path().join("owner.pid");
        let status = Command::new(std::env::current_exe()?)
            .arg(OWNER_FIXTURE)
            .arg(&pid_file)
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "owner fixture failed: {status:?}"
            )));
        }
        let pid = wait_for_pid(&pid_file, Duration::from_secs(5))?;
        wait_until_process_gone(pid, Duration::from_secs(5))
    }

    fn membership_fixture() -> io::Result<()> {
        let mut in_job = 0;
        if unsafe { IsProcessInJob(GetCurrentProcess(), null_mut(), &raw mut in_job) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if in_job == 0 {
            return Err(io::Error::other(
                "first fixture instruction ran outside a Job Object",
            ));
        }
        Ok(())
    }

    fn nested_fixture(pid_file: &Path) -> io::Result<()> {
        let child = Command::new(std::env::current_exe()?)
            .arg(LINGER_FIXTURE)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        fs::write(pid_file, format!("{}\n", child.id()))?;
        // Do not wait: the inherited Job membership must retain this nested
        // process after the direct payload exits.
        drop(child);
        Ok(())
    }

    fn owner_fixture(pid_file: &Path) -> io::Result<()> {
        let mut command = ContainmentCommand::new(std::env::current_exe()?);
        command
            .arg(LINGER_FIXTURE)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let process = factory("abrupt-owner")?.spawn(command)?;
        fs::write(pid_file, format!("{}\n", process.id()))?;
        std::mem::forget(process);
        // Process teardown closes the sole Job handle. This deliberately
        // bypasses the Rust cleanup coordinator to test KILL_ON_JOB_CLOSE.
        std::process::exit(0);
    }

    fn factory(identity: &str) -> io::Result<temper_process_containment::PreparedContainment> {
        let backend: Arc<dyn ContainmentBackendFactory> = Arc::new(WindowsJobBackendFactory);
        let factory = ContainmentFactory::new(ContainmentBackendPolicy::RequireWindowsJob, backend);
        factory.prepare(
            ContainmentSpec::new(ContainmentIdentity::new(identity)?, ContainmentScope::Tool)
                .with_timing(Duration::from_millis(20), Duration::from_millis(10)),
        )
    }

    fn assert_empty(report: &temper_process_containment::CleanupReport) -> io::Result<()> {
        if report.backend() != temper_process_containment::ContainmentBackendKind::WindowsJob {
            return Err(io::Error::other(
                "Windows factory selected the wrong backend",
            ));
        }
        if !matches!(report.recursive_empty(), RecursiveEmptyProof::Proven { .. }) {
            return Err(io::Error::other("Windows Job was not proven empty"));
        }
        if !report.direct_child_reap().is_terminal() {
            return Err(io::Error::other(
                "Windows cleanup omitted direct-process wait status",
            ));
        }
        Ok(())
    }

    fn one_path(arguments: &[std::ffi::OsString]) -> io::Result<&Path> {
        let [path] = arguments else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fixture requires exactly one path",
            ));
        };
        Ok(Path::new(path))
    }

    fn wait_for_pid(path: &Path, timeout: Duration) -> io::Result<u32> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(value) = fs::read_to_string(path) {
                if let Ok(pid) = value.trim().parse() {
                    return Ok(pid);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out waiting for {}", path.display()),
        ))
    }

    fn wait_until_process_gone(pid: u32, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let raw = unsafe { OpenProcess(SYNCHRONIZE_PROCESS, 0, pid) };
            if raw.is_null() {
                return Ok(());
            }
            // SAFETY: OpenProcess returned this uniquely owned handle.
            let process = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
            if unsafe { WaitForSingleObject(process.as_raw_handle().cast(), 0) } != WAIT_TIMEOUT {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("process {pid} survived Job cleanup"),
        ))
    }
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    windows::main()
}

#[cfg(not(windows))]
fn main() {}
