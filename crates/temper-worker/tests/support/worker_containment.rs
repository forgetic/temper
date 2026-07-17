#[cfg(target_os = "linux")]
mod linux {
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitCode, Stdio};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use temper_process_containment::{
        CleanupDisposition, ContainmentBackendFactory, ContainmentBackendPolicy,
        ContainmentFactory, LinuxSupervisorBackendFactory, dispatch_linux_supervisor_helper,
    };
    use temper_protocol_agent::{WorkspaceContext, WorkspaceRepository, WorkspaceWorkItem};
    use temper_worker::{
        AgentRunRequest, AgentRunner, AttemptFence, JobCancellation, JobProgressReporter,
        OutOfProcessRunner, WorkerLivenessLimits,
    };

    const CONTRACT_TEST: &str = "worker_descendant_containment_contract";

    pub fn main() -> ExitCode {
        if let Some(status) = dispatch_linux_supervisor_helper(std::env::args_os().skip(1)) {
            return status;
        }
        let arguments: Vec<_> = std::env::args_os().skip(1).collect();
        if arguments.iter().any(|argument| argument == "--list") {
            if !arguments.iter().any(|argument| argument == "--ignored") {
                println!("{CONTRACT_TEST}: test");
            }
            return ExitCode::SUCCESS;
        }
        match run_contract() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("worker descendant containment contract failed: {error}");
                ExitCode::FAILURE
            }
        }
    }

    fn run_contract() -> io::Result<()> {
        run_case("normal", 0, true)?;
        run_case("failure", 17, false)
    }

    fn run_case(name: &str, exit_code: i32, expect_success: bool) -> io::Result<()> {
        let temporary = tempfile::tempdir()?;
        let script = write_agent_fixture(temporary.path())?;
        let pid_file = temporary.path().join(format!("{name}.pid"));
        let helper = std::env::current_exe()?;
        let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
            .with_env(vec![
                (
                    "TEMPER_DESCENDANT_PID".to_string(),
                    pid_file.display().to_string(),
                ),
                ("TEMPER_AGENT_EXIT".to_string(), exit_code.to_string()),
            ])
            .with_liveness_limits(WorkerLivenessLimits {
                forced_termination_grace: Duration::from_millis(100),
                ..WorkerLivenessLimits::default()
            })
            .with_containment_factory(move |_job, _attempt| {
                let backend: Arc<dyn ContainmentBackendFactory> = Arc::new(
                    LinuxSupervisorBackendFactory::with_helper_executable(helper.clone()),
                );
                Ok(ContainmentFactory::new(
                    ContainmentBackendPolicy::ForceLinuxSupervisor,
                    backend,
                ))
            });
        let context = context();
        let cancellation = JobCancellation::default();
        let run_cancellation = cancellation.clone();
        let run_name = name.to_string();
        let cwd = temporary.path().to_path_buf();
        let result = temper_worker_io::block_on(async move {
            runner
                .run_request(AgentRunRequest::new_controlled(
                    &run_name,
                    format!("attempt-{run_name}"),
                    &context,
                    &cwd,
                    AttemptFence::open(),
                    run_cancellation,
                    JobProgressReporter::noop(format!("attempt-{run_name}")),
                ))
                .await
        });
        if result.is_ok() != expect_success {
            return Err(io::Error::other(format!(
                "unexpected {name} agent result: {result:?}"
            )));
        }

        let cleanup = cancellation
            .cleanup()
            .ok_or_else(|| io::Error::other("agent run omitted its cleanup proof"))?;
        if !cleanup.proves_quiescence() {
            return Err(io::Error::other(format!(
                "agent run returned an unproven cleanup report: {cleanup:?}"
            )));
        }
        if cleanup.containment.disposition() != CleanupDisposition::Killed {
            return Err(io::Error::other(format!(
                "nested session did not require hard-kill cleanup: {:?}",
                cleanup.containment.disposition()
            )));
        }
        if cleanup.containment.observed_survivors().is_empty() {
            return Err(io::Error::other(
                "cleanup report omitted the recovered descendant identity",
            ));
        }
        let pid = wait_for_pid(&pid_file, Duration::from_secs(2))?;
        wait_until_gone(pid, Duration::from_secs(2))
    }

    fn write_agent_fixture(directory: &Path) -> io::Result<PathBuf> {
        let script = directory.join("nested-agent.sh");
        fs::write(
            &script,
            r#"#!/bin/sh
set -eu
if [ "${1:-}" = "descendant" ]; then
    echo "$$" > "$2"
    trap '' TERM
    while :; do sleep 1; done
fi
result=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --result) result="$2"; shift 2 ;;
        *) shift ;;
    esac
done
setsid "$0" descendant "$TEMPER_DESCENDANT_PID" </dev/null >/dev/null 2>&1 &
limit=0
while [ ! -s "$TEMPER_DESCENDANT_PID" ] && [ "$limit" -lt 200 ]; do
    sleep 0.01
    limit=$((limit + 1))
done
printf '{"summary":"nested fixture completed"}' > "$result"
exit "$TEMPER_AGENT_EXIT"
"#,
        )?;
        let mut permissions = fs::metadata(&script)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions)?;
        Ok(script)
    }

    fn wait_for_pid(path: &Path, timeout: Duration) -> io::Result<u32> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(value) = fs::read_to_string(path) {
                return value
                    .trim()
                    .parse()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "nested fixture did not publish its pid",
        ))
    }

    fn wait_until_gone(pid: u32, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success()
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("nested descendant {pid} survived job completion"),
        ))
    }

    fn context() -> WorkspaceContext {
        WorkspaceContext {
            trace_context: None,
            artifact_context: None,
            repos: vec![WorkspaceRepository {
                id: "acme/svc".to_string(),
                owner: "acme".to_string(),
                name: "svc".to_string(),
                default_branch: "main".to_string(),
                dir: "svc".to_string(),
                access: "writable".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("temper/fixture".to_string()),
            }],
            work_item: WorkspaceWorkItem {
                role: "engineer".to_string(),
                queue: "code".to_string(),
                kind: "issue".to_string(),
                target: "Issue { number: ItemNumber(453) }".to_string(),
                context: "{}".to_string(),
            },
            action: "open_pr".to_string(),
            correlation_key: "worker-containment-fixture".to_string(),
            checkout: Some("writable".to_string()),
            allowed_verdicts: Vec::new(),
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: Default::default(),
            pull_request_freshness: None,
            agent_session: None,
        }
    }
}

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::SUCCESS
}
