use super::support::*;

#[cfg(unix)]
#[derive(Clone)]
struct PausingCommitAgent {
    entered: PathBuf,
    release: PathBuf,
    late_mutation: PathBuf,
    pid: PathBuf,
}

#[cfg(unix)]
impl AgentRunner for PausingCommitAgent {
    async fn run(
        &self,
        _job_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> Result<AgentRunOutput, AgentRunError> {
        use std::os::unix::fs::PermissionsExt as _;

        let repo = context.primary().expect("primary repo");
        let repo_root = cwd.join(&repo.dir);
        fs::write(repo_root.join("agent-output.txt"), "agent diff\n")
            .expect("write fake agent diff");
        let hook = repo_root.join(".git/hooks/pre-commit");
        let script = format!(
            "#!/bin/sh\nset -eu\nprintf '%s' \"$$\" > {}\ntouch {}\nwhile [ ! -e {} ]; do sleep 0.01; done\nprintf late > {}\n",
            shell_quote(&self.pid),
            shell_quote(&self.entered),
            shell_quote(&self.release),
            shell_quote(&self.late_mutation),
        );
        fs::write(&hook, script).expect("write pausing commit hook");
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&hook, permissions).unwrap();

        let fingerprint = temper_worker::fingerprint_writable_repos(context, cwd)
            .await
            .map_err(|error| AgentRunError::transient(format!("fingerprint submit: {error}")))?;
        Ok(AgentRunOutput::with_accepted_submit(
            WorkspaceResult {
                summary: Some("pause during commit".to_string()),
                ..WorkspaceResult::default()
            },
            temper_worker::AcceptedSubmitProof {
                response: temper_protocol_agent::SubmitForPrResponse::accepted("accepted"),
                fingerprint,
            },
        ))
    }
}

#[cfg(unix)]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

#[cfg(unix)]
fn process_exists(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(unix)]
fn wait_for_path(path: &Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        std::thread::yield_now();
    }
}

#[test]
#[cfg(unix)]
fn cancelled_paused_commit_joins_before_next_capacity_one_job() {
    let fixture = Fixture::new();
    let control = fixture.workspace_root.join("commit-control");
    fs::create_dir_all(&control).expect("commit control dir");
    let entered = control.join("entered");
    let release = control.join("release");
    let late_mutation = control.join("late-mutation");
    let pid = control.join("pid");
    let executor = fixture.executor(
        PausingCommitAgent {
            entered: entered.clone(),
            release: release.clone(),
            late_mutation: late_mutation.clone(),
            pid: pid.clone(),
        },
        true,
    );
    let assignment = assign("agent/pr-for-code-7", "pr-for-code-7");
    let mut machine = temper_worker::WorkerMachine::new(temper_worker::WorkerParams {
        worker_id: "capacity-one-worker".to_string(),
        worker_pool: None,
        capabilities: vec![temper_worker::CapabilitySpec {
            repo: "acme/service".to_string(),
            role: "engineer".to_string(),
        }],
        max_concurrent_jobs: 1,
        poll_wait: std::time::Duration::from_millis(10),
        heartbeat_interval: std::time::Duration::from_millis(10),
        poll_backoff: std::time::Duration::from_millis(10),
        liveness_limits: temper_worker::WorkerLivenessLimits {
            max_no_progress: std::time::Duration::from_nanos(10),
            ..Default::default()
        },
        result_root: fixture.workspace_root.join("results"),
    });
    let dispatched = temper_worker_io::Machine::on_completion(
        &mut machine,
        temper_worker_io::EngineTime::ZERO,
        temper_worker::WorkerCompletion::PollReply(Ok(Some(
            temper_protocol_worker::WorkerProtocolMessage::Assign(assignment.clone()),
        ))),
    );
    let generation = dispatched
        .iter()
        .find_map(|request| match request {
            temper_worker::WorkerRequest::RunJob { generation, .. } => Some(*generation),
            _ => None,
        })
        .expect("first capacity-one job dispatched");
    assert_eq!(machine.free_capacity(), 0);
    let timer_generation = machine
        .job_state(&assignment.job_id)
        .expect("first job watch state")
        .timer_generation;

    let execution = temper_worker::JobExecutionContext::unsupervised(&assignment);
    let cancellation = execution.cancellation.clone();
    let fence = execution.fence.clone();
    let attempt_assignment = assignment.clone();
    let attempt = std::thread::spawn(move || {
        temper_worker_io::block_on(async move {
            temper_worker::JobExecutor::execute(&executor, attempt_assignment, execution).await
        })
    });

    wait_for_path(&entered);
    let timeout = temper_worker_io::Machine::on_completion(
        &mut machine,
        temper_worker_io::EngineTime::from_nanos(11),
        temper_worker::WorkerCompletion::WatchdogTimer {
            job_id: assignment.job_id.clone(),
            attempt_id: assignment.attempt_id.clone().expect("attempt id"),
            generation,
            timer_generation,
            kind: temper_worker::WatchdogTimerKind::NoProgress,
        },
    );
    assert!(timeout.iter().any(|request| matches!(
        request,
        temper_worker::WorkerRequest::CancelJob { job_id, .. } if job_id == &assignment.job_id
    )));
    fence.close();
    cancellation.cancel();
    let outcome = attempt.join().expect("join cancelled coding attempt");

    let message = expect_failure_class(outcome, FailureClass::Transient);
    assert!(message.contains("cancel"), "unexpected message: {message}");
    let hook_pid = fs::read_to_string(pid).expect("paused hook pid");
    assert!(
        !process_exists(&hook_pid),
        "attempt completed before the paused commit process was reaped"
    );
    fs::write(&release, b"").expect("release hypothetical detached commit");
    assert!(
        !late_mutation.exists(),
        "commit hook mutated the workspace after cancellation quiesced"
    );
    assert_no_origin_branch(&fixture, "agent/pr-for-code-7");

    let checkout = fixture
        .workspace_root
        .join("engineer/pr-for-code-7/service");
    assert!(
        checkout.join("agent-output.txt").exists(),
        "watchdog cancellation must preserve dirty coordination-scoped work"
    );
    let session = temper_worker::AgentSessionStore::for_workspace_root(
        &fixture.workspace_root,
        "engineer",
        "pr-for-code-7",
    )
    .expect("session store");
    assert!(
        session.path().exists(),
        "watchdog cancellation must preserve the coordination session"
    );

    let record = temper_worker_io::Machine::on_completion(
        &mut machine,
        temper_worker_io::EngineTime::from_nanos(12),
        temper_worker::WorkerCompletion::JobQuiesced {
            job_id: assignment.job_id.clone(),
            attempt_id: assignment.attempt_id.clone().expect("attempt id"),
            generation,
            cleanup: temper_worker::JobCleanup {
                cancellation: temper_worker::CancellationOutcome::Graceful,
                descendants: temper_worker::DescendantCleanupStatus::Clean,
            },
        },
    );
    assert_eq!(machine.free_capacity(), 0, "durability precedes release");
    let timeout_result = record
        .iter()
        .find_map(|request| match request {
            temper_worker::WorkerRequest::RecordResult { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("timeout result recorded only after git joins");
    let entry = temper_worker::ResultOutboxEntry::from_result(timeout_result)
        .expect("timeout outbox entry");
    let released = temper_worker_io::Machine::on_completion(
        &mut machine,
        temper_worker_io::EngineTime::from_nanos(13),
        temper_worker::WorkerCompletion::ResultRecorded {
            job_id: assignment.job_id.clone(),
            attempt_id: assignment.attempt_id.clone().expect("attempt id"),
            generation,
            outcome: Ok(entry),
        },
    );
    assert_eq!(machine.free_capacity(), 1);
    assert!(
        released
            .iter()
            .any(|request| matches!(request, temper_worker::WorkerRequest::SendPoll(_)))
    );

    // The capacity-one successor is dispatched only after the first attempt's
    // joined owner and durable timeout result released the permit.
    let next_assignment = assign("agent/pr-for-code-8", "pr-for-code-8");
    let next_dispatch = temper_worker_io::Machine::on_completion(
        &mut machine,
        temper_worker_io::EngineTime::from_nanos(14),
        temper_worker::WorkerCompletion::PollReply(Ok(Some(
            temper_protocol_worker::WorkerProtocolMessage::Assign(next_assignment.clone()),
        ))),
    );
    assert!(next_dispatch.iter().any(|request| matches!(
        request,
        temper_worker::WorkerRequest::RunJob { assign, .. }
            if assign.job_id == next_assignment.job_id
    )));
    let next = fixture.executor(AgentBehavior::Success.runner(), true);
    let outcome = temper_worker_io::block_on(async move { next.execute(next_assignment).await });
    let (branch, _, _) = expect_success(outcome);
    assert_eq!(branch, "agent/pr-for-code-8");
    assert!(!late_mutation.exists());
    assert_no_origin_branch(&fixture, "agent/pr-for-code-7");
}
