#[cfg(target_os = "linux")]
mod linux {
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitCode, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use temper_agent_core::{AgentContainmentContext, ManagedBashTool};
    use temper_process_containment::{
        CgroupV2BackendFactory, CgroupV2FactoryConfig, CleanupDisposition, CleanupObserver,
        CleanupReport, CleanupSnapshot, CleanupTrigger, ContainmentBackendFactory,
        ContainmentBackendKind, ContainmentBackendPolicy, ContainmentFactory,
        LinuxSupervisorBackendFactory, RecursiveEmptyProof, dispatch_linux_supervisor_helper,
    };
    use temper_protocol_agent::{WorkspaceContext, WorkspaceRepository, WorkspaceWorkItem};
    use temper_testing::descendant_fixture::{
        current_exact_identity, read_recorded_identities, unique_identities,
    };
    use temper_worker::{
        AgentRunRequest, AgentRunner, AttemptFence, JobCancellation, JobProgressReporter,
        OutOfProcessRunner, PrePushStatus, WorkerLivenessLimits,
        run_managed_worker_command_for_acceptance, run_pre_push_checks_for_acceptance,
    };
    use tongs::tools::Tool as _;

    const MUTATION_SETTLE: Duration = Duration::from_millis(75);

    #[path = "worker_lifecycle.rs"]
    mod worker_lifecycle;

    #[derive(Clone, Copy, Debug)]
    enum BackendMode {
        ForcedSupervisor,
        AutoCgroup,
    }

    impl BackendMode {
        fn label(self) -> &'static str {
            match self {
                Self::ForcedSupervisor => "forced-supervisor",
                Self::AutoCgroup => "auto-cgroup-v2",
            }
        }
    }

    #[derive(Default)]
    struct RecordingObserver(Mutex<Vec<CleanupReport>>);

    impl CleanupObserver for RecordingObserver {
        fn observe(&self, snapshot: &CleanupSnapshot) {
            if let CleanupSnapshot::Completed { report } = snapshot {
                self.0
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(report.clone());
            }
        }
    }

    impl RecordingObserver {
        fn one_report(&self) -> CleanupReport {
            let reports = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(reports.len(), 1, "cleanup must execute exactly once");
            reports[0].clone()
        }
    }

    struct FixtureCase {
        temporary: tempfile::TempDir,
        fixture: PathBuf,
        identities: PathBuf,
        ready: PathBuf,
        mutation_trigger: PathBuf,
        late_mutation: PathBuf,
        monitor_report: PathBuf,
        monitor_stop: PathBuf,
        monitor: Option<Child>,
    }

    impl FixtureCase {
        fn start(fixture: &Path, name: &str) -> io::Result<Self> {
            let temporary = tempfile::Builder::new().prefix(name).tempdir()?;
            let identities = temporary.path().join("identities.tsv");
            let ready = temporary.path().join("ready");
            let mutation_trigger = temporary.path().join("attempt-late-mutation");
            let late_mutation = temporary.path().join("late-workspace-mutation");
            let monitor_report = temporary.path().join("monitor-report");
            let monitor_stop = temporary.path().join("monitor-stop");
            let monitor = Command::new(fixture)
                .arg("monitor")
                .arg(&identities)
                .arg(&monitor_report)
                .arg(&monitor_stop)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()?;
            Ok(Self {
                temporary,
                fixture: fixture.to_path_buf(),
                identities,
                ready,
                mutation_trigger,
                late_mutation,
                monitor_report,
                monitor_stop,
                monitor: Some(monitor),
            })
        }

        fn detached_shell_command(&self) -> String {
            format!(
                "setsid {} parent {} {} {} {} true >/dev/null 2>&1 & \
                 while [ ! -e {} ]; do sleep 0.01; done",
                quote(&self.fixture),
                quote(&self.identities),
                quote(&self.ready),
                quote(&self.mutation_trigger),
                quote(&self.late_mutation),
                quote(&self.ready),
            )
        }

        fn foreground_shell_command(&self) -> String {
            format!(
                "exec setsid {} parent {} {} {} {} true",
                quote(&self.fixture),
                quote(&self.identities),
                quote(&self.ready),
                quote(&self.mutation_trigger),
                quote(&self.late_mutation),
            )
        }

        fn agent_command(&self, exit_code: i32, hold: bool) -> Vec<String> {
            vec![
                self.fixture.display().to_string(),
                "agent".to_string(),
                self.identities.display().to_string(),
                self.ready.display().to_string(),
                self.mutation_trigger.display().to_string(),
                self.late_mutation.display().to_string(),
                "true".to_string(),
                exit_code.to_string(),
                hold.to_string(),
            ]
        }

        fn finish(&mut self, minimum_identities: usize, mode: BackendMode) -> io::Result<()> {
            let records = unique_identities(read_recorded_identities(&self.identities)?);
            if records.len() < minimum_identities {
                return Err(io::Error::other(format!(
                    "fixture published only {} identities: {records:?}",
                    records.len()
                )));
            }
            for identity in &records {
                if current_exact_identity(identity)?.is_some() {
                    return Err(io::Error::other(format!(
                        "completion preceded exact identity absence: {identity:?}"
                    )));
                }
            }

            // Give an escaped fixture an explicit opportunity to perform its
            // post-completion mutation. A correctly joined tree cannot react.
            fs::write(&self.mutation_trigger, b"mutate now\n")?;
            std::thread::sleep(MUTATION_SETTLE);
            if self.late_mutation.exists() {
                return Err(io::Error::other(
                    "fixture mutated the workspace after completion",
                ));
            }

            fs::write(&self.monitor_stop, b"stop\n")?;
            let status = self
                .monitor
                .take()
                .expect("monitor is present until finish")
                .wait()?;
            if !status.success() {
                return Err(io::Error::other(format!("monitor failed with {status}")));
            }
            let report = fs::read_to_string(&self.monitor_report)?;
            for expected in [
                format!("identity_count={}", records.len()),
                "alive=0".to_string(),
            ] {
                if !report.lines().any(|line| line == expected) {
                    return Err(io::Error::other(format!(
                        "monitor omitted `{expected}`: {report}"
                    )));
                }
            }
            // The dedicated Linux supervisor is a subreaper and must prevent
            // every PPID-1 transition. A delegated cgroup remains the kernel
            // ownership boundary even while an exiting root's descendants are
            // transiently reparented, so its authority is exact absence and
            // recursive-empty proof rather than process parentage.
            if matches!(mode, BackendMode::ForcedSupervisor)
                && !report.lines().any(|line| line == "orphaned=0")
            {
                return Err(io::Error::other(format!(
                    "monitor omitted `orphaned=0`: {report}"
                )));
            }
            Ok(())
        }
    }

    impl Drop for FixtureCase {
        fn drop(&mut self) {
            if let Some(mut monitor) = self.monitor.take() {
                let _ = fs::write(&self.monitor_stop, b"stop\n");
                for _ in 0..25 {
                    if monitor.try_wait().ok().flatten().is_some() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                let _ = monitor.kill();
                let _ = monitor.wait();
            }
        }
    }

    pub fn main() -> ExitCode {
        if let Some(status) = dispatch_linux_supervisor_helper(std::env::args_os().skip(1)) {
            return status;
        }
        let fixture = match std::env::args_os().nth(1) {
            Some(path) => PathBuf::from(path),
            None => {
                eprintln!("acceptance driver requires the compiled fixture path");
                return ExitCode::FAILURE;
            }
        };
        match run(&fixture) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("descendant-containment acceptance failed: {error}");
                ExitCode::FAILURE
            }
        }
    }

    fn run(fixture: &Path) -> io::Result<()> {
        run_backend_suite(BackendMode::ForcedSupervisor, fixture)?;
        println!("BACKEND forced-supervisor PASS");

        match cgroup_capability()? {
            None => {
                run_backend_suite(BackendMode::AutoCgroup, fixture)?;
                println!("BACKEND auto-cgroup-v2 PASS");
            }
            Some(reason) => println!("CGROUP SKIP: {reason}"),
        }
        Ok(())
    }

    fn run_backend_suite(mode: BackendMode, fixture: &Path) -> io::Result<()> {
        managed_bash_success(mode, fixture)?;
        managed_bash_deadline(mode, fixture)?;
        out_of_process_agent(mode, fixture, 0, true)?;
        out_of_process_agent(mode, fixture, 17, false)?;
        out_of_process_cancellation(mode, fixture)?;
        worker_lifecycle::watchdog_capacity_one(mode, fixture)?;
        println!("CASE {} capacity-one-watchdog PASS", mode.label());
        worker_lifecycle::inspection_failure_retains_capacity(mode, fixture)?;
        println!("CASE {} inspection-recovery PASS", mode.label());
        worker_lifecycle::signal_shutdown(mode, fixture, false)?;
        println!("CASE {} split-signal-shutdown PASS", mode.label());
        worker_lifecycle::signal_shutdown(mode, fixture, true)?;
        println!("CASE {} standalone-signal-shutdown PASS", mode.label());
        worker_managed_command(mode, fixture)?;
        println!("CASE {} worker-managed-command PASS", mode.label());
        run_pre_push_case(mode, fixture)?;
        println!("CASE {} pre-push PASS", mode.label());
        Ok(())
    }

    fn managed_bash_success(mode: BackendMode, fixture: &Path) -> io::Result<()> {
        let mut case = FixtureCase::start(fixture, &format!("bash-success-{}", mode.label()))?;
        let observer = Arc::new(RecordingObserver::default());
        let context = containment_context(mode, "bash-success", observer.clone())?;
        let command = case.detached_shell_command();
        let output = temper_testing::block_on(async {
            ManagedBashTool::with_containment(case.temporary.path(), context)
                .execute(
                    "detached-success",
                    serde_json::json!({"command": command}),
                    None,
                )
                .await
        })
        .map_err(|error| io::Error::other(error.to_string()))?;
        if output.is_error {
            return Err(io::Error::other(
                "managed bash direct success became an error",
            ));
        }
        assert_recovered_cleanup(&observer.one_report(), mode, CleanupTrigger::NormalRootExit)?;
        case.finish(2, mode)
    }

    fn managed_bash_deadline(mode: BackendMode, fixture: &Path) -> io::Result<()> {
        let mut case = FixtureCase::start(fixture, &format!("bash-timeout-{}", mode.label()))?;
        let observer = Arc::new(RecordingObserver::default());
        let context = containment_context(mode, "bash-timeout", observer.clone())?;
        let command = case.foreground_shell_command();
        let output = temper_testing::block_on(async {
            ManagedBashTool::with_containment(case.temporary.path(), context)
                .execute(
                    "tool-deadline",
                    serde_json::json!({"command": command, "timeout": 1}),
                    None,
                )
                .await
        })
        .map_err(|error| io::Error::other(error.to_string()))?;
        if !output.is_error {
            return Err(io::Error::other("managed bash deadline reported success"));
        }
        assert_recovered_cleanup(&observer.one_report(), mode, CleanupTrigger::Timeout)?;
        case.finish(2, mode)
    }

    fn out_of_process_agent(
        mode: BackendMode,
        fixture: &Path,
        exit_code: i32,
        expect_success: bool,
    ) -> io::Result<()> {
        let mut case = FixtureCase::start(fixture, &format!("agent-{exit_code}-{}", mode.label()))?;
        fs::create_dir(case.temporary.path().join("demo"))?;
        let runner = runner(mode, case.agent_command(exit_code, false));
        let cancellation = JobCancellation::default();
        let context = workspace_context();
        let result = temper_testing::block_on(runner.run_request(AgentRunRequest::new_controlled(
            "agent-fixture",
            format!("attempt-{exit_code}"),
            &context,
            case.temporary.path(),
            AttemptFence::open(),
            cancellation.clone(),
            JobProgressReporter::noop(format!("attempt-{exit_code}")),
        )));
        if result.is_ok() != expect_success {
            return Err(io::Error::other(format!(
                "unexpected agent result: {result:?}"
            )));
        }
        let cleanup = cancellation
            .cleanup()
            .ok_or_else(|| io::Error::other("agent omitted its joined cleanup report"))?;
        if !cleanup.proves_quiescence() {
            return Err(io::Error::other(format!(
                "agent cleanup was not proven: {cleanup:?}"
            )));
        }
        assert_recovered_cleanup(&cleanup.containment, mode, CleanupTrigger::NormalRootExit)?;
        case.finish(3, mode)
    }

    fn out_of_process_cancellation(mode: BackendMode, fixture: &Path) -> io::Result<()> {
        let mut case = FixtureCase::start(fixture, &format!("agent-cancel-{}", mode.label()))?;
        fs::create_dir(case.temporary.path().join("demo"))?;
        let runner = runner(mode, case.agent_command(0, true));
        let cancellation = JobCancellation::default();
        let cancel = cancellation.clone();
        let ready = case.ready.clone();
        let requester = std::thread::spawn(move || {
            wait_for_path(&ready, Duration::from_secs(3)).expect("fixture became ready");
            cancel.hard_kill();
        });
        let context = workspace_context();
        let result = temper_testing::block_on(runner.run_request(AgentRunRequest::new_controlled(
            "cancel-fixture",
            "attempt-cancel",
            &context,
            case.temporary.path(),
            AttemptFence::open(),
            cancellation.clone(),
            JobProgressReporter::noop("attempt-cancel"),
        )));
        requester
            .join()
            .map_err(|_| io::Error::other("cancellation requester panicked"))?;
        if result.is_ok() {
            return Err(io::Error::other("cancelled agent result was accepted"));
        }
        let cleanup = cancellation
            .cleanup()
            .ok_or_else(|| io::Error::other("cancelled agent omitted cleanup"))?;
        if !cleanup.proves_quiescence() || cleanup.cancellation.is_none() {
            return Err(io::Error::other(format!(
                "invalid cancellation cleanup: {cleanup:?}"
            )));
        }
        assert_recovered_cleanup(&cleanup.containment, mode, CleanupTrigger::Watchdog)?;
        case.finish(3, mode)
    }

    fn worker_managed_command(mode: BackendMode, fixture: &Path) -> io::Result<()> {
        let mut case = FixtureCase::start(fixture, &format!("worker-command-{}", mode.label()))?;
        let mut command = Command::new("/bin/bash");
        command.args(["-c", &case.detached_shell_command()]);
        let cancellation = JobCancellation::with_containment_factory(factory(
            mode,
            "worker-command",
            "acceptance",
            None,
        )?);
        let status = temper_testing::block_on(run_managed_worker_command_for_acceptance(
            command,
            cancellation,
        ))?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "worker-managed fixture command failed with {status}"
            )));
        }
        case.finish(2, mode)
    }

    fn run_pre_push_case(mode: BackendMode, fixture: &Path) -> io::Result<()> {
        let mut case = FixtureCase::start(fixture, &format!("pre-push-{}", mode.label()))?;
        fs::create_dir(case.temporary.path().join(".temper"))?;
        let command = case.detached_shell_command();
        let config = format!(
            "version = 1\n\n[pre_push]\nrequired = true\ncwd = \"repo\"\n\n\
             [[pre_push.commands]]\nid = \"descendant-fixture\"\nargv = [\"/bin/bash\", \"-c\", {}]\ntimeout_secs = 5\n",
            toml_string(&command),
        );
        fs::write(case.temporary.path().join(".temper/pre-push.toml"), config)?;
        let cancellation = JobCancellation::with_containment_factory(factory(
            mode,
            "pre-push",
            "acceptance",
            None,
        )?);
        let report = temper_testing::block_on(run_pre_push_checks_for_acceptance(
            case.temporary.path(),
            cancellation,
        ))
        .map_err(|error| io::Error::other(error.to_string()))?;
        if report.status != PrePushStatus::Passed || report.commands.len() != 1 {
            return Err(io::Error::other(format!(
                "unexpected pre-push report: {report:?}"
            )));
        }
        case.finish(2, mode)
    }

    fn runner(mode: BackendMode, command: Vec<String>) -> OutOfProcessRunner {
        OutOfProcessRunner::new(command)
            .with_liveness_limits(WorkerLivenessLimits {
                graceful_cancellation_grace: Duration::from_millis(40),
                forced_termination_grace: Duration::from_millis(40),
                ..WorkerLivenessLimits::default()
            })
            .with_containment_factory(move |job, attempt| factory(mode, job, attempt, None))
    }

    fn containment_context(
        mode: BackendMode,
        owner: &str,
        observer: Arc<dyn CleanupObserver>,
    ) -> io::Result<AgentContainmentContext> {
        Ok(
            AgentContainmentContext::new(factory(mode, owner, "acceptance", Some(observer))?, None)
                .with_cleanup_timing(Duration::from_millis(40), Duration::from_millis(5)),
        )
    }

    fn factory(
        mode: BackendMode,
        job: &str,
        attempt: &str,
        observer: Option<Arc<dyn CleanupObserver>>,
    ) -> io::Result<ContainmentFactory> {
        let (policy, backend) = backend_factory(mode, job, attempt)?;
        let factory = ContainmentFactory::new(policy, backend);
        Ok(observer.map_or(factory.clone(), |observer| factory.with_observer(observer)))
    }

    fn backend_factory(
        mode: BackendMode,
        job: &str,
        attempt: &str,
    ) -> io::Result<(ContainmentBackendPolicy, Arc<dyn ContainmentBackendFactory>)> {
        let helper = std::env::current_exe()?;
        let supervisor: Arc<dyn ContainmentBackendFactory> = Arc::new(
            LinuxSupervisorBackendFactory::with_helper_executable(helper),
        );
        match mode {
            BackendMode::ForcedSupervisor => {
                Ok((ContainmentBackendPolicy::ForceLinuxSupervisor, supervisor))
            }
            BackendMode::AutoCgroup => {
                let config = CgroupV2FactoryConfig::new(job, attempt)?;
                let backend: Arc<dyn ContainmentBackendFactory> =
                    Arc::new(CgroupV2BackendFactory::system(config).with_fallback(supervisor));
                Ok((ContainmentBackendPolicy::Auto, backend))
            }
        }
    }

    fn cgroup_capability() -> io::Result<Option<String>> {
        let config =
            CgroupV2FactoryConfig::new(format!("acceptance-{}", std::process::id()), "capability")?;
        let factory = CgroupV2BackendFactory::system(config);
        Ok((!factory.capability().delegation_available()).then(|| {
            factory
                .capability()
                .diagnostic()
                .unwrap_or("delegated cgroup-v2 requirements are unavailable")
                .to_string()
        }))
    }

    fn assert_recovered_cleanup(
        report: &CleanupReport,
        mode: BackendMode,
        trigger: CleanupTrigger,
    ) -> io::Result<()> {
        let expected_backend = match mode {
            BackendMode::ForcedSupervisor => ContainmentBackendKind::LinuxSupervisor,
            BackendMode::AutoCgroup => ContainmentBackendKind::LinuxCgroupV2,
        };
        if report.backend() != expected_backend
            || report.trigger() != trigger
            || report.disposition() == CleanupDisposition::AlreadyEmpty
            || report.term_attempts().is_empty()
            || report.observed_survivors().is_empty()
            || !matches!(report.recursive_empty(), RecursiveEmptyProof::Proven { .. })
        {
            return Err(io::Error::other(format!(
                "cleanup evidence did not prove signal survivor recovery: {report:?}"
            )));
        }
        if report.observed_survivors().iter().any(|identity| {
            identity.start_time_identity() == 0 || identity.executable().as_os_str().is_empty()
        }) {
            return Err(io::Error::other(
                "cleanup survivor evidence omitted start-time or executable identity",
            ));
        }
        Ok(())
    }

    fn workspace_context() -> WorkspaceContext {
        WorkspaceContext {
            trace_context: None,
            artifact_context: None,
            repos: vec![WorkspaceRepository {
                id: "forgejo:ai/fixture".to_string(),
                owner: "ai".to_string(),
                name: "fixture".to_string(),
                default_branch: "main".to_string(),
                dir: "demo".to_string(),
                access: "writable".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("agent/containment-fixture".to_string()),
            }],
            work_item: WorkspaceWorkItem {
                role: "engineer".to_string(),
                queue: "code_ready".to_string(),
                kind: "code".to_string(),
                target: "Issue { number: ItemNumber(457) }".to_string(),
                context: "{}".to_string(),
            },
            action: "open_pr".to_string(),
            correlation_key: "descendant-containment-acceptance".to_string(),
            checkout: Some("writable".to_string()),
            allowed_verdicts: Vec::new(),
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: Default::default(),
            pull_request_freshness: None,
            agent_session: None,
        }
    }

    fn wait_for_path(path: &Path, timeout: Duration) -> io::Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if path.exists() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out waiting for {}", path.display()),
        ))
    }

    fn quote(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    fn toml_string(value: &str) -> String {
        format!("{value:?}")
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
