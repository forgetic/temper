use super::*;
use std::process::{Command as StdCommand, Stdio as StdStdio};
use std::time::Instant;

static PROCESS_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(target_os = "linux")]
struct CleanupErrorObserver(Arc<AtomicBool>);

#[cfg(target_os = "linux")]
impl temper_process_containment::CleanupObserver for CleanupErrorObserver {
    fn observe(&self, snapshot: &temper_process_containment::CleanupSnapshot) {
        if matches!(
            snapshot,
            temper_process_containment::CleanupSnapshot::Blocked { .. }
        ) {
            self.0.store(true, Ordering::Release);
        }
    }
}

#[cfg(target_os = "linux")]
struct CleanupErrorFactory;

#[cfg(target_os = "linux")]
impl temper_process_containment::ContainmentBackendFactory for CleanupErrorFactory {
    fn prepare_backend(
        &self,
        policy: temper_process_containment::ContainmentBackendPolicy,
        spec: &temper_process_containment::ContainmentSpec,
    ) -> std::io::Result<Box<dyn temper_process_containment::PreparedContainmentBackend>> {
        let executable = std::env::current_exe()?;
        let inner =
            temper_process_containment::LinuxSupervisorBackendFactory::with_helper_invocation(
                executable,
                [
                    "--exact",
                    "containment_tests::linux_supervisor_helper_entrypoint",
                    "--nocapture",
                ],
            )
            .prepare_backend(policy, spec)?;
        Ok(Box::new(CleanupErrorPrepared { inner }))
    }
}

#[cfg(target_os = "linux")]
struct CleanupErrorPrepared {
    inner: Box<dyn temper_process_containment::PreparedContainmentBackend>,
}

#[cfg(target_os = "linux")]
impl temper_process_containment::PreparedContainmentBackend for CleanupErrorPrepared {
    fn kind(&self) -> temper_process_containment::ContainmentBackendKind {
        self.inner.kind()
    }

    fn root_identity(&self) -> temper_process_containment::ContainmentRootIdentity {
        self.inner.root_identity()
    }

    fn spawn_precontained(
        self: Box<Self>,
        command: ContainmentCommand,
    ) -> std::io::Result<temper_process_containment::BackendSpawn> {
        self.inner.spawn_precontained(command).map(|spawned| {
            spawned.map_kernel(|inner| {
                Box::new(CleanupErrorKernel {
                    inner,
                    injected_error: false,
                })
            })
        })
    }
}

#[cfg(target_os = "linux")]
struct CleanupErrorKernel {
    inner: Box<dyn temper_process_containment::ContainmentKernel>,
    injected_error: bool,
}

#[cfg(target_os = "linux")]
impl temper_process_containment::ContainmentKernel for CleanupErrorKernel {
    fn backend_kind(&self) -> temper_process_containment::ContainmentBackendKind {
        self.inner.backend_kind()
    }

    fn root_identity(&self) -> temper_process_containment::ContainmentRootIdentity {
        self.inner.root_identity()
    }

    fn discover_members(&mut self) -> std::io::Result<temper_process_containment::MemberDiscovery> {
        if !self.injected_error {
            self.injected_error = true;
            return Err(std::io::Error::other("injected cleanup inspection failure"));
        }
        self.inner.discover_members()
    }

    fn signal_members(
        &mut self,
        signal: temper_process_containment::ContainmentSignal,
    ) -> std::io::Result<temper_process_containment::SignalBatch> {
        self.inner.signal_members(signal)
    }

    fn take_backend_signal_batch(
        &mut self,
        signal: temper_process_containment::ContainmentSignal,
    ) -> Option<temper_process_containment::SignalBatch> {
        self.inner.take_backend_signal_batch(signal)
    }

    fn reap_direct_child(
        &mut self,
        child: &mut std::process::Child,
    ) -> std::io::Result<temper_process_containment::DirectChildReap> {
        self.inner.reap_direct_child(child)
    }

    fn verify_recursive_empty(
        &mut self,
    ) -> std::io::Result<temper_process_containment::RecursiveEmptyProof> {
        self.inner.verify_recursive_empty()
    }

    fn wait(&mut self, duration: Duration) {
        self.inner.wait(duration);
    }
}

fn managed(dir: &Path) -> ManagedBashTool {
    ManagedBashTool::with_containment(dir, crate::containment_tests::containment_context())
}

fn detached_command(pid_file: &Path, keep_root: bool) -> String {
    let wait = if keep_root { "wait" } else { "true" };
    format!(
        "setsid sh -c 'echo $$ > \"{}\"; trap \"\" TERM; while :; do sleep 1; done' </dev/null >/dev/null 2>&1 & \
         while [ ! -s \"{}\" ]; do sleep 0.01; done; {wait}",
        pid_file.display(),
        pid_file.display(),
    )
}

fn wait_for_pid(path: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(value) = std::fs::read_to_string(path) {
            if let Ok(pid) = value.trim().parse() {
                return pid;
            }
        }
        assert!(Instant::now() < deadline, "detached pid was not published");
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let exists = StdCommand::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(StdStdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !exists {
        return false;
    }
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rsplit_once(") ").map(|(_, rest)| rest.to_string()))
        .and_then(|rest| rest.chars().next())
        .is_some_and(|state| state != 'Z')
}

fn text(output: &ToolOutput) -> &str {
    match &output.content[0] {
        tongs::model::ContentBlock::Text(text) => &text.text,
        other => panic!("expected text output, got {other:?}"),
    }
}

#[test]
#[cfg(target_os = "linux")]
fn normal_exit_waits_for_detached_session_cleanup_and_reader_join() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("detached.pid");
    let output = temper_agent_io::block_on({
        let tool = managed(dir.path());
        let command = detached_command(&pid_file, false);
        async move {
            tool.execute("normal", serde_json::json!({"command": command}), None)
                .await
                .expect("managed bash")
        }
    });
    let pid = wait_for_pid(&pid_file);
    assert!(
        !process_alive(pid),
        "terminal output preceded cleanup proof"
    );
    assert_eq!(ACTIVE_OUTPUT_READERS.load(Ordering::Acquire), 0);
    assert!(!output.is_error, "direct shell exit remains successful");
}

#[test]
#[cfg(target_os = "linux")]
fn explicit_tool_timeout_waits_for_cleanup_and_reader_join() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("timeout.pid");
    let context = crate::containment_tests::containment_context();
    let command = detached_command(&pid_file, true);
    let mut task = ManagedBashTask::spawn(
        dir.path().to_path_buf(),
        "timeout",
        BashInput {
            command,
            timeout: Some(60),
        },
        context,
    )
    .expect("spawn managed task");
    let pid = wait_for_pid(&pid_file);
    let output = temper_agent_io::block_on(async move {
        assert!(
            temper_agent_io::timeout(Duration::from_millis(100), &mut task)
                .await
                .is_err()
        );
        task.timeout();
        task.await.expect("timeout output")
    });
    assert!(!process_alive(pid), "timeout result preceded cleanup proof");
    assert_eq!(ACTIVE_OUTPUT_READERS.load(Ordering::Acquire), 0);
    assert!(output.is_error);
    assert!(text(&output).contains("timed out"));
}

#[test]
#[cfg(target_os = "linux")]
fn generic_cancellation_hands_cleanup_to_a_dedicated_owner() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("cancel.pid");
    let outcome = temper_agent_io::block_on({
        let tool = managed(dir.path());
        let command = detached_command(&pid_file, true);
        async move {
            temper_agent_io::timeout(
                Duration::from_millis(500),
                tool.execute("cancel", serde_json::json!({"command": command}), None),
            )
            .await
        }
    });
    assert!(outcome.is_err(), "generic cancellation must win");
    let pid = wait_for_pid(&pid_file);
    wait_for_process_exit(pid);
    wait_for_output_readers();
}

#[test]
#[cfg(target_os = "linux")]
fn direct_task_drop_does_not_block_while_cleanup_joins() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("drop.pid");
    let task = ManagedBashTask::spawn(
        dir.path().to_path_buf(),
        "drop",
        BashInput {
            command: detached_command(&pid_file, true),
            timeout: None,
        },
        crate::containment_tests::containment_context(),
    )
    .expect("spawn managed task");
    let pid = wait_for_pid(&pid_file);
    drop(task);
    wait_for_process_exit(pid);
    wait_for_output_readers();
}

#[cfg(target_os = "linux")]
fn wait_for_process_exit(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while process_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "cleanup owner did not reap {pid}"
        );
        thread::yield_now();
    }
}

#[cfg(target_os = "linux")]
fn wait_for_output_readers() {
    let deadline = Instant::now() + Duration::from_secs(2);
    while ACTIVE_OUTPUT_READERS.load(Ordering::Acquire) != 0 {
        assert!(Instant::now() < deadline, "output reader did not join");
        thread::yield_now();
    }
}

#[test]
#[cfg(target_os = "linux")]
fn cleanup_inspection_error_blocks_output_until_recovery_and_reader_join() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let blocked = Arc::new(AtomicBool::new(false));
    let factory = temper_process_containment::ContainmentFactory::new(
        temper_process_containment::ContainmentBackendPolicy::ForceLinuxSupervisor,
        Arc::new(CleanupErrorFactory),
    )
    .with_observer(Arc::new(CleanupErrorObserver(Arc::clone(&blocked))));
    let context = AgentContainmentContext::new(factory, None)
        .with_cleanup_timing(Duration::from_millis(5), Duration::from_millis(1));
    let mut task = ManagedBashTask::spawn(
        tempfile::tempdir().expect("tempdir").path().to_path_buf(),
        "cleanup-error",
        BashInput {
            command: "sleep 60".to_string(),
            timeout: Some(60),
        },
        context,
    )
    .expect("spawn managed task");
    let output = temper_agent_io::block_on(async move {
        assert!(
            temper_agent_io::timeout(Duration::from_millis(50), &mut task)
                .await
                .is_err()
        );
        task.timeout();
        task.await.expect("cleanup recovers")
    });
    assert!(blocked.load(Ordering::Acquire));
    assert!(output.is_error);
    assert_eq!(ACTIVE_OUTPUT_READERS.load(Ordering::Acquire), 0);
}

#[test]
fn rendering_preserves_success_and_tail_bounds() {
    let (output, is_error) = render_outcome("hello\n", false, Some(0), None);
    assert_eq!(output, "hello\n");
    assert!(!is_error);

    let content = (1..=20_000)
        .map(|number| number.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let (output, lines, truncated) = truncate_tail(&content);
    assert!(truncated);
    assert!(lines <= MAX_OUTPUT_LINES);
    assert!(output.len() <= MAX_OUTPUT_BYTES);
    assert!(output.contains("20000"));
    assert!(!output.contains("\n1\n"));
}

#[test]
fn schema_preserves_the_tongs_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let managed = ManagedBashTool::new(dir.path());
    let tongs = tongs::tools::create_bash_tool(dir.path());
    assert_eq!(managed.name(), tongs.name());
    assert_eq!(managed.description(), tongs.description());
    assert_eq!(managed.parameters(), tongs.parameters());
    assert_eq!(managed.effects(), tongs.effects());
}

#[test]
fn text_accessor_fixture_is_sound() {
    let output = ToolOutput::text("fixture");
    assert_eq!(text(&output), "fixture");
}
