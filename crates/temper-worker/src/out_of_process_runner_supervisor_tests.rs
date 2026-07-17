use std::future::Future as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityEventV1, RunFinishedV1, RunStatusV1,
};
use temper_protocol_agent::{AgentRuntimeLimitsV1, SubmitForPrResponse};

use super::supervisor::{ManagedAgentProcess, SupervisorResult};
use super::{DiagnosticIdentity, OutOfProcessRunner, tests::test_context};
use crate::agent_runner::{
    AgentForgeContextHost, AgentRunRequest, AgentRunner, JobProgressReporter,
};
use crate::config::{WorkerAgentTraceConfig, WorkerLivenessLimits};
use crate::executor::{AttemptFence, CancellationOutcome, JobCancellation};
use crate::trace::TraceCollector;

fn executable_script(directory: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let path = directory.join(name);
    std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn managed(script: &Path) -> ManagedAgentProcess {
    let command = Command::new(script);
    let contained = crate::process_containment::containment_command(
        &command,
        Stdio::null(),
        Stdio::null(),
        Stdio::piped(),
    );
    let factory = crate::process_containment::production_factory("supervisor-test", "attempt")
        .expect("test containment factory");
    let prepared = factory
        .prepare(
            temper_process_containment::ContainmentSpec::new(
                temper_process_containment::ContainmentIdentity::new("managed-agent-test")
                    .expect("identity"),
                temper_process_containment::ContainmentScope::Job,
            )
            .with_timing(Duration::from_millis(50), Duration::from_millis(5)),
        )
        .expect("prepare managed child");
    ManagedAgentProcess::spawn(
        prepared,
        contained,
        DiagnosticIdentity::from_context("supervisor-test", &test_context()),
        tracing::dispatcher::get_default(|dispatch| dispatch.clone()),
    )
    .expect("spawn managed child")
}

#[test]
#[cfg(unix)]
fn cooperative_window_observes_a_graceful_child_exit_and_joins_stderr() {
    let temp = tempfile::tempdir().unwrap();
    let script = executable_script(
        temp.path(),
        "graceful.sh",
        "printf 'joined stderr marker\\n' >&2\nsleep 0.05\nexit 0",
    );
    let mut process = managed(&script);
    assert!(process.request_cancel());
    let result = wait_for_supervisor(&mut process);

    assert_eq!(
        result.quiesced.cleanup.cancellation,
        Some(CancellationOutcome::Graceful)
    );
    let outcome = result.outcome.unwrap();
    assert_eq!(outcome.status_code, Some(0));
    assert!(outcome.stderr_tail.contains("joined stderr marker"));
}

#[test]
#[cfg(unix)]
fn unresponsive_child_escalates_to_hard_kill_without_a_lingering_waiter() {
    let temp = tempfile::tempdir().unwrap();
    let ready = temp.path().join("ready");
    let script = executable_script(
        temp.path(),
        "hard-kill.sh",
        &format!(
            "trap '' TERM\n: > '{}'\nwhile :; do sleep 1; done",
            ready.display()
        ),
    );
    let mut process = managed(&script);
    wait_for_file(&ready);
    let started = Instant::now();
    assert!(process.request_cancel());
    assert!(process.force_terminate());
    assert!(process.hard_kill());
    let result = wait_for_supervisor(&mut process);

    assert_eq!(
        result.quiesced.cleanup.cancellation,
        Some(CancellationOutcome::HardKill)
    );
    assert_eq!(
        result.quiesced.cleanup.containment.disposition(),
        temper_process_containment::CleanupDisposition::Killed,
        "{:?}",
        result.quiesced
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    // cancel_and_join consumed the supervisor thread's result and joined it;
    // dropping the owner cannot leave a blocking Child::wait behind.
    drop(process);
}

#[test]
#[cfg(unix)]
fn cancellation_kills_and_reaps_a_child_process_group_grandchild() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("grandchild.pid");
    let script = executable_script(
        temp.path(),
        "grandchild.sh",
        &format!(
            "trap '' TERM\n(sleep 30) &\necho $! > '{}'\nwait",
            pid_file.display()
        ),
    );
    let mut process = managed(&script);
    wait_for_file(&pid_file);
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    let pid = pid.trim();
    assert!(process.request_cancel());
    assert!(process.force_terminate());
    assert!(process.hard_kill());
    let _ = wait_for_supervisor(&mut process);

    for _ in 0..50 {
        if !process_exists(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("grandchild {pid} survived process-group hard kill");
}

#[test]
#[cfg(unix)]
fn hung_forge_host_does_not_block_child_exit_or_accepted_socket_shutdown() {
    let temp = tempfile::tempdir().unwrap();
    let connected = temp.path().join("forge-connected");
    let script = executable_script(
        temp.path(),
        "hung-forge.sh",
        r#"
forge=""; result=""
while [ "$#" -gt 0 ]; do
  arg="$1"; shift
  case "$arg" in
    --forge-context-address) forge="$1"; shift ;;
    --result) result="$1"; shift ;;
    --context|--workspace|--runtime-limits|--agent-lifecycle-address|--activity-address|--trace-policy|--tool-config|--submit-for-pr-address) shift ;;
  esac
done
python3 - "$forge" <<'PY' &
import json, socket, sys
address = sys.argv[1]
host, port = address.rsplit(':', 1)
stream = socket.create_connection((host, int(port)), timeout=5)
request = {"protocol_version":1,"operation":{"operation":"forge_get_item","repo":"acme/svc","number":7,"type":"issue","include_comments":False}}
stream.sendall(json.dumps(request).encode())
stream.shutdown(socket.SHUT_WR)
while stream.recv(1024): pass
PY
while [ ! -e "${TEMPER_CONNECTED:?}" ]; do sleep 0.01; done
printf '{"summary":"ok"}' > "$result"
"#,
    );
    let host_started = Arc::new(AtomicBool::new(false));
    let host_started_for_call = Arc::clone(&host_started);
    let connected_for_host = connected.clone();
    let host: AgentForgeContextHost = Arc::new(move |_job_id, _operation| {
        host_started_for_call.store(true, Ordering::Release);
        std::fs::write(&connected_for_host, b"").expect("mark host invocation");
        Box::pin(std::future::pending())
    });
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_env(vec![(
            "TEMPER_CONNECTED".to_string(),
            connected.display().to_string(),
        )])
        .with_runtime_limits(Some(AgentRuntimeLimitsV1::default()))
        .with_forge_context_host(host)
        .with_liveness_limits(short_limits());
    let context = test_context();
    let cwd = temp.path().to_path_buf();
    let started = Instant::now();
    let output =
        temper_worker_io::block_on(async move { runner.run("hung-forge", &context, &cwd).await })
            .expect("child exit wins over a hung Forge host");

    assert_eq!(output.result.summary.as_deref(), Some("ok"));
    assert!(host_started.load(Ordering::Acquire));
    assert!(started.elapsed() < Duration::from_secs(2));
}

struct PendingSubmit {
    dropped: Arc<AtomicBool>,
}

impl Future for PendingSubmit {
    type Output = SubmitForPrResponse;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl Drop for PendingSubmit {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

#[test]
#[cfg(unix)]
fn hung_submit_host_is_dropped_and_joined_before_run_cancellation_returns() {
    let temp = tempfile::tempdir().unwrap();
    let requested = temp.path().join("submit-requested");
    let script = executable_script(
        temp.path(),
        "hung-submit.sh",
        r#"
submit=""
while [ "$#" -gt 0 ]; do
  arg="$1"; shift
  case "$arg" in
    --submit-for-pr-address) submit="$1"; shift ;;
    --context|--result|--workspace|--runtime-limits|--agent-lifecycle-address|--activity-address|--trace-policy|--tool-config|--forge-context-address) shift ;;
  esac
done
python3 - "$submit" "${TEMPER_REQUESTED:?}" <<'PY'
import json, socket, sys
address, requested = sys.argv[1:]
host, port = address.rsplit(':', 1)
stream = socket.create_connection((host, int(port)), timeout=5)
request = {"protocol_version":1,"correlation_key":"test-key","role":"engineer","action":"open_pr","summary":"ready"}
stream.sendall(json.dumps(request).encode())
stream.shutdown(socket.SHUT_WR)
open(requested, 'w').close()
while stream.recv(1024): pass
PY
"#,
    );
    let host_dropped = Arc::new(AtomicBool::new(false));
    let host_started = Arc::new(AtomicBool::new(false));
    let dropped_for_host = Arc::clone(&host_dropped);
    let started_for_host = Arc::clone(&host_started);
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_env(vec![(
            "TEMPER_REQUESTED".to_string(),
            requested.display().to_string(),
        )])
        .with_async_submit_for_pr_handler(move |_request, _context, _cwd| {
            started_for_host.store(true, Ordering::Release);
            PendingSubmit {
                dropped: Arc::clone(&dropped_for_host),
            }
        })
        .with_liveness_limits(short_limits());
    let context = test_context();
    let cwd = temp.path().to_path_buf();
    let mut future = Box::pin(runner.run("hung-submit", &context, &cwd));
    let mut task_context = Context::from_waker(Waker::noop());
    assert!(matches!(
        future.as_mut().poll(&mut task_context),
        Poll::Pending
    ));
    wait_for_file(&requested);
    for _ in 0..100 {
        if host_started.load(Ordering::Acquire) {
            break;
        }
        assert!(matches!(
            future.as_mut().poll(&mut task_context),
            Poll::Pending
        ));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        host_started.load(Ordering::Acquire),
        "submit host did not start"
    );

    let started = Instant::now();
    drop(future);
    assert!(host_dropped.load(Ordering::Acquire));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
#[cfg(unix)]
fn connected_first_party_child_receives_cancel_before_process_escalation() {
    let temp = tempfile::tempdir().unwrap();
    let ready = temp.path().join("lifecycle-ready");
    let cancelled = temp.path().join("lifecycle-cancelled");
    let script = executable_script(
        temp.path(),
        "cooperative-cancel.sh",
        r#"
lifecycle=""
while [ "$#" -gt 0 ]; do
  arg="$1"; shift
  case "$arg" in
    --agent-lifecycle-address) lifecycle="$1"; shift ;;
    --context|--result|--workspace|--runtime-limits|--tool-config|--trace-policy|--activity-address|--submit-for-pr-address|--forge-context-address) shift ;;
  esac
done
python3 - "$lifecycle" "${TEMPER_READY:?}" "${TEMPER_CANCELLED:?}" <<'PY'
import json, socket, sys
address, ready, cancelled = sys.argv[1:]
host, port = address.rsplit(':', 1)
stream = socket.create_connection((host, int(port)), timeout=5)
stream.sendall(b'{"version":1}\n')
open(ready, 'w').close()
command = b''
while not command.endswith(b'\n'):
    command += stream.recv(1024)
assert json.loads(command)['command'] == 'cancel'
stream.sendall(b'{"version":1}\n')
open(cancelled, 'w').close()
stream.shutdown(socket.SHUT_WR)
PY
"#,
    );
    let trace_config = WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(temp.path().join("graceful-cancel-spool")),
    };
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_env(vec![
            ("TEMPER_READY".to_string(), ready.display().to_string()),
            (
                "TEMPER_CANCELLED".to_string(),
                cancelled.display().to_string(),
            ),
        ])
        .with_runtime_limits(Some(AgentRuntimeLimitsV1::default()))
        .with_trace_collector(trace_config.clone())
        .with_liveness_limits(WorkerLivenessLimits {
            graceful_cancellation_grace: Duration::from_millis(500),
            forced_termination_grace: Duration::from_millis(50),
            ..Default::default()
        });
    let context = test_context();
    let cwd = temp.path().to_path_buf();
    let cancellation = JobCancellation::default();
    let fence = AttemptFence::open();
    let request = AgentRunRequest::new_controlled(
        "cooperative-cancel",
        "attempt-cooperative-cancel",
        &context,
        &cwd,
        fence.clone(),
        cancellation.clone(),
        JobProgressReporter::noop("attempt-cooperative-cancel"),
    );
    let mut future = Box::pin(runner.run_request(request));
    let mut task_context = Context::from_waker(Waker::noop());
    assert!(matches!(
        future.as_mut().poll(&mut task_context),
        Poll::Pending
    ));
    wait_for_file(&ready);

    let started = Instant::now();
    fence.close();
    cancellation.cancel();
    let _ = poll_until_ready(future.as_mut());
    assert!(cancelled.exists(), "child did not receive lifecycle Cancel");
    assert_eq!(
        cancellation.cleanup().unwrap().cancellation,
        Some(CancellationOutcome::Graceful)
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_cancelled_terminal(&trace_config);
}

#[test]
#[cfg(unix)]
fn hard_kill_writes_synthetic_cancelled_terminal_activity_and_reports_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let ready = temp.path().join("forced-ready");
    let script = executable_script(
        temp.path(),
        "forced-cancel.sh",
        &format!(
            "trap '' TERM\n: > '{}'\nwhile :; do sleep 1; done",
            ready.display()
        ),
    );
    let trace_config = WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(temp.path().join("forced-cancel-spool")),
    };
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_trace_collector(trace_config.clone())
        .with_liveness_limits(short_limits());
    let context = test_context();
    let cwd = temp.path().to_path_buf();
    let cancellation = JobCancellation::default();
    let fence = AttemptFence::open();
    let request = AgentRunRequest::new_controlled(
        "forced-cancel",
        "attempt-forced-cancel",
        &context,
        &cwd,
        fence.clone(),
        cancellation.clone(),
        JobProgressReporter::noop("attempt-forced-cancel"),
    );
    let mut future = Box::pin(runner.run_request(request));
    let mut task_context = Context::from_waker(Waker::noop());
    assert!(matches!(
        future.as_mut().poll(&mut task_context),
        Poll::Pending
    ));
    wait_for_file(&ready);
    fence.close();
    cancellation.cancel();
    cancellation.force_terminate();
    cancellation.hard_kill();
    let _ = poll_until_ready(future.as_mut());
    let cleanup = cancellation.cleanup().expect("supervisor cleanup report");
    assert_eq!(cleanup.cancellation, Some(CancellationOutcome::HardKill));
    assert_eq!(
        cleanup.containment.disposition(),
        temper_process_containment::CleanupDisposition::Killed
    );
    assert_cancelled_terminal(&trace_config);
}

#[test]
#[cfg(unix)]
fn forced_termination_fences_late_result_and_reports_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let ready = temp.path().join("ready");
    let late_copy = temp.path().join("late-result-copy.json");
    let script = executable_script(
        temp.path(),
        "late-result.sh",
        r#"
result=""
while [ "$#" -gt 0 ]; do
  arg="$1"; shift
  case "$arg" in
    --result) result="$1"; shift ;;
    --context|--workspace|--tool-config|--trace-policy|--activity-address|--submit-for-pr-address|--forge-context-address) shift ;;
  esac
done
trap 'printf "{\"summary\":\"late\"}" > "$result"; cp "$result" "${TEMPER_LATE_COPY:?}"; exit 0' TERM
: > "${TEMPER_READY:?}"
while :; do sleep 1; done
"#,
    );
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_env(vec![
            ("TEMPER_READY".to_string(), ready.display().to_string()),
            (
                "TEMPER_LATE_COPY".to_string(),
                late_copy.display().to_string(),
            ),
        ])
        .with_liveness_limits(short_limits());
    let context = test_context();
    let cwd = temp.path().to_path_buf();
    let cancellation = JobCancellation::default();
    let fence = AttemptFence::open();
    let request = AgentRunRequest::new_controlled(
        "late-result",
        "attempt-late-result",
        &context,
        &cwd,
        fence.clone(),
        cancellation.clone(),
        JobProgressReporter::noop("attempt-late-result"),
    );
    let mut future = Box::pin(runner.run_request(request));
    let mut task_context = Context::from_waker(Waker::noop());
    assert!(matches!(
        future.as_mut().poll(&mut task_context),
        Poll::Pending
    ));
    wait_for_file(&ready);

    fence.close();
    cancellation.cancel();
    cancellation.force_terminate();
    let _ = poll_until_ready(future.as_mut());
    assert_eq!(
        cancellation.cleanup().unwrap().cancellation,
        Some(CancellationOutcome::ForcedTermination)
    );
    assert_eq!(
        std::fs::read_to_string(late_copy).unwrap(),
        "{\"summary\":\"late\"}"
    );
}

fn poll_until_ready<F: Future>(mut future: std::pin::Pin<&mut F>) -> F::Output {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let mut task_context = Context::from_waker(Waker::noop());
        if let Poll::Ready(output) = future.as_mut().poll(&mut task_context) {
            return output;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for controlled agent cancellation"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wait_for_supervisor(process: &mut ManagedAgentProcess) -> SupervisorResult {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let mut task_context = Context::from_waker(Waker::noop());
        if let Poll::Ready(result) = process.poll_outcome(&mut task_context) {
            process.join_completed();
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for managed process supervisor"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn assert_cancelled_terminal(config: &WorkerAgentTraceConfig) {
    let recovered = TraceCollector::new(config.clone()).recover().unwrap();
    assert_eq!(recovered.len(), 1);
    assert!(matches!(
        recovered[0].events.last().map(|event| &event.event),
        Some(AgentActivityEventV1::RunFinished(RunFinishedV1 {
            status: RunStatusV1::Cancelled,
            ..
        }))
    ));
}

fn short_limits() -> WorkerLivenessLimits {
    WorkerLivenessLimits {
        graceful_cancellation_grace: Duration::from_millis(50),
        forced_termination_grace: Duration::from_millis(50),
        ..Default::default()
    }
}

fn wait_for_file(path: &Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {}", path.display());
}

fn process_exists(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
