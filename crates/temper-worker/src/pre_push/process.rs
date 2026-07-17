use std::future::Future;
use std::io::{self, Read};
use std::path::PathBuf;
use std::pin::Pin;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use temper_process_containment::{CleanupTrigger, ContainedProcess, ContainmentScope};

use super::PrePushCommand;
use crate::executor::{JobCancellation, JobCleanupObserver};

const OUTPUT_TAIL_BYTES: usize = 8 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Structured data for a single configured command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PrePushCommandResult {
    pub id: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    pub exit_code: Option<i32>,
    pub exit_status: Option<String>,
    pub timed_out: bool,
    pub elapsed_ms: u64,
    pub stdout_tail: String,
    pub stderr_tail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PrePushCommandResult {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && self.error.is_none()
    }
}

pub(super) async fn run_command(
    command: PrePushCommand,
    cwd: PathBuf,
    cancellation: Option<JobCancellation>,
) -> PrePushCommandResult {
    ManagedPrePushCommand::spawn(command, cwd, cancellation).await
}

struct ManagedCommandState {
    result: Option<PrePushCommandResult>,
    waker: Option<Waker>,
}

/// Joined owner for one worker-side gate command. Dropping a submit request
/// cancels the process group and joins its waiter/readers before the attempt can
/// report quiescence.
struct ManagedPrePushCommand {
    state: Arc<Mutex<ManagedCommandState>>,
    cancelled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ManagedPrePushCommand {
    fn spawn(command: PrePushCommand, cwd: PathBuf, cancellation: Option<JobCancellation>) -> Self {
        let state = Arc::new(Mutex::new(ManagedCommandState {
            result: None,
            waker: None,
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_cancelled = Arc::clone(&cancelled);
        let fallback_command = command.clone();
        let fallback_cwd = cwd.clone();
        let owner_command = command.clone();
        let owner_cwd = cwd.clone();
        let thread = match thread::Builder::new()
            .name("temper-pre-push-command".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_command_sync(command, cwd, &thread_cancelled, cancellation)
                }))
                .unwrap_or_else(|_| {
                    spawn_error_result(
                        owner_command,
                        owner_cwd,
                        io::Error::other("pre-push command owner panicked"),
                    )
                });
                let waker = {
                    let mut state = thread_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.result = Some(result);
                    state.waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            }) {
            Ok(thread) => Some(thread),
            Err(error) => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .result = Some(spawn_error_result(fallback_command, fallback_cwd, error));
                None
            }
        };
        Self {
            state,
            cancelled,
            thread,
        }
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Future for ManagedPrePushCommand {
    type Output = PrePushCommandResult;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.result.is_none()
                && !state
                    .waker
                    .as_ref()
                    .is_some_and(|waker| waker.will_wake(cx.waker()))
            {
                state.waker = Some(cx.waker().clone());
            }
            state.result.take()
        };
        match result {
            Some(result) => {
                self.join();
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for ManagedPrePushCommand {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.join();
    }
}

fn spawn_error_result(
    command: PrePushCommand,
    cwd: PathBuf,
    error: io::Error,
) -> PrePushCommandResult {
    PrePushCommandResult {
        id: command.id,
        argv: command.argv,
        cwd,
        timeout_secs: command.timeout_secs,
        exit_code: None,
        exit_status: None,
        timed_out: false,
        elapsed_ms: 0,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
        error: Some(format!("start command owner: {error}")),
    }
}

fn run_command_sync(
    command: PrePushCommand,
    cwd: PathBuf,
    cancelled: &AtomicBool,
    cancellation: Option<JobCancellation>,
) -> PrePushCommandResult {
    let started = Instant::now();
    let mut result = PrePushCommandResult {
        id: command.id,
        argv: command.argv,
        cwd,
        timeout_secs: command.timeout_secs,
        exit_code: None,
        exit_status: None,
        timed_out: false,
        elapsed_ms: 0,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
        error: None,
    };

    let Some((program, args)) = result.argv.split_first() else {
        result.error = Some("empty argv".to_string());
        result.elapsed_ms = elapsed_ms(started);
        return result;
    };

    let mut process = Command::new(program);
    process.args(args).current_dir(&result.cwd);
    let contained_command = crate::process_containment::containment_command(
        &process,
        Stdio::null(),
        Stdio::piped(),
        Stdio::piped(),
    );
    let observer = cancellation.map(|cancellation| {
        Arc::new(JobCleanupObserver(cancellation))
            as Arc<dyn temper_process_containment::CleanupObserver>
    });
    let prepared = match crate::process_containment::prepare_with_observer(
        "pre-push",
        "local",
        ContainmentScope::PrePush,
        &result.id,
        observer,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            result.error = Some(format!("prepare process containment: {error}"));
            result.elapsed_ms = elapsed_ms(started);
            return result;
        }
    };
    let child = match prepared.spawn(contained_command) {
        Ok(child) => child,
        Err(error) => {
            result.error = Some(format!("spawn: {error}"));
            result.elapsed_ms = elapsed_ms(started);
            return result;
        }
    };

    let stdout = match child.take_stdout() {
        Ok(Some(stdout)) => stdout,
        Ok(None) => {
            let _ = child.cleanup(CleanupTrigger::Shutdown);
            result.error = Some("stdout was not piped".to_string());
            result.elapsed_ms = elapsed_ms(started);
            return result;
        }
        Err(error) => {
            let _ = child.cleanup(CleanupTrigger::Shutdown);
            result.error = Some(format!("take stdout: {error}"));
            result.elapsed_ms = elapsed_ms(started);
            return result;
        }
    };
    let stderr = match child.take_stderr() {
        Ok(Some(stderr)) => stderr,
        Ok(None) => {
            let _ = child.cleanup(CleanupTrigger::Shutdown);
            result.error = Some("stderr was not piped".to_string());
            result.elapsed_ms = elapsed_ms(started);
            return result;
        }
        Err(error) => {
            let _ = child.cleanup(CleanupTrigger::Shutdown);
            result.error = Some(format!("take stderr: {error}"));
            result.elapsed_ms = elapsed_ms(started);
            return result;
        }
    };
    let stdout_reader = match spawn_reader(stdout, "stdout") {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.cleanup(CleanupTrigger::Shutdown);
            set_error(&mut result, error.to_string());
            result.elapsed_ms = elapsed_ms(started);
            return result;
        }
    };
    let stderr_reader = match spawn_reader(stderr, "stderr") {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.cleanup(CleanupTrigger::Shutdown);
            let _ = stdout_reader.join();
            set_error(&mut result, error.to_string());
            result.elapsed_ms = elapsed_ms(started);
            return result;
        }
    };

    match wait_with_timeout(&child, Duration::from_secs(result.timeout_secs), cancelled) {
        Ok(outcome) => {
            result.timed_out = outcome.timed_out;
            if outcome.cancelled {
                result.error = Some("cancelled by agent attempt".to_string());
            }
            if let Some(status) = outcome.status {
                result.exit_code = status.code();
                result.exit_status = Some(status.to_string());
            }
        }
        Err(error) => {
            result.error = Some(format!("wait: {error}"));
            let _ = child.cleanup(CleanupTrigger::Shutdown);
        }
    }

    // A gate may exit after backgrounding a descendant that still owns an
    // output pipe. Recursive emptiness is proven before stream readers join.
    let _cleanup = child.cleanup(CleanupTrigger::NormalRootExit);

    match join_reader(stdout_reader, "stdout") {
        Ok(output) => result.stdout_tail = tail_utf8(&output, OUTPUT_TAIL_BYTES),
        Err(error) => set_error(&mut result, error),
    }
    match join_reader(stderr_reader, "stderr") {
        Ok(output) => result.stderr_tail = tail_utf8(&output, OUTPUT_TAIL_BYTES),
        Err(error) => set_error(&mut result, error),
    }
    result.elapsed_ms = elapsed_ms(started);
    result
}

struct WaitOutcome {
    status: Option<ExitStatus>,
    timed_out: bool,
    cancelled: bool,
}

fn wait_with_timeout(
    child: &ContainedProcess,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> io::Result<WaitOutcome> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait_root()? {
            return Ok(WaitOutcome {
                status: Some(status),
                timed_out: false,
                cancelled: false,
            });
        }
        if cancelled.load(Ordering::Acquire) {
            let cleanup = child.cleanup(CleanupTrigger::Cancellation);
            return Ok(WaitOutcome {
                status: exit_status_from_cleanup(&cleanup),
                timed_out: false,
                cancelled: true,
            });
        }
        if started.elapsed() >= timeout {
            let cleanup = child.cleanup(CleanupTrigger::Timeout);
            return Ok(WaitOutcome {
                status: exit_status_from_cleanup(&cleanup),
                timed_out: true,
                cancelled: false,
            });
        }
        thread::sleep(timeout.saturating_sub(started.elapsed()).min(POLL_INTERVAL));
    }
}

fn exit_status_from_cleanup(
    _cleanup: &temper_process_containment::CleanupReport,
) -> Option<ExitStatus> {
    // Cleanup retains the portable exit code in its structured direct-child
    // proof. `ExitStatus` has no portable constructor, and timeout/cancellation
    // results do not require one to decide success.
    None
}

fn spawn_reader<R>(
    mut reader: R,
    stream: &'static str,
) -> io::Result<thread::JoinHandle<io::Result<Vec<u8>>>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name(format!("temper-pre-push-{stream}"))
        .spawn(move || read_tail(&mut reader, OUTPUT_TAIL_BYTES))
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("start pre-push {stream} reader: {error}"),
            )
        })
}

fn read_tail(reader: &mut impl Read, max_len: usize) -> io::Result<Vec<u8>> {
    let mut tail = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(tail);
        }
        append_tail(&mut tail, &buffer[..read], max_len);
    }
}

fn append_tail(tail: &mut Vec<u8>, chunk: &[u8], max_len: usize) {
    if chunk.len() >= max_len {
        tail.clear();
        tail.extend_from_slice(&chunk[chunk.len() - max_len..]);
        return;
    }
    let overflow = tail
        .len()
        .saturating_add(chunk.len())
        .saturating_sub(max_len);
    if overflow > 0 {
        tail.drain(..overflow);
    }
    tail.extend_from_slice(chunk);
}

fn join_reader(
    handle: thread::JoinHandle<io::Result<Vec<u8>>>,
    stream: &'static str,
) -> Result<Vec<u8>, String> {
    match handle.join() {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(format!("read {stream}: {error}")),
        Err(_panic) => Err(format!("read {stream}: reader thread panicked")),
    }
}

fn set_error(result: &mut PrePushCommandResult, message: String) {
    match &mut result.error {
        Some(error) => {
            error.push_str("; ");
            error.push_str(&message);
        }
        None => result.error = Some(message),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn tail_utf8(bytes: &[u8], max_len: usize) -> String {
    let text = String::from_utf8_lossy(bytes).into_owned();
    if text.len() <= max_len {
        return text;
    }
    let mut start = text.len() - max_len;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;
    use std::task::{Wake, Waker};

    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    #[test]
    #[cfg(unix)]
    fn cancellation_joins_pre_push_before_late_workspace_mutation() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let script = temporary.path().join("pre-push-cancel.sh");
        let pid = temporary.path().join("pid");
        let entered = temporary.path().join("entered");
        let release = temporary.path().join("release");
        let late = temporary.path().join("late");
        std::fs::write(
            &script,
            "#!/bin/sh\nset -eu\nprintf '%s' \"$$\" > \"$1\"\ntouch \"$2\"\nwhile [ ! -e \"$3\" ]; do sleep 0.01; done\nprintf late > \"$4\"\n",
        )
        .expect("write fixture");
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();

        let command = PrePushCommand {
            id: "cancel-fixture".to_string(),
            argv: vec![
                script.display().to_string(),
                pid.display().to_string(),
                entered.display().to_string(),
                release.display().to_string(),
                late.display().to_string(),
            ],
            timeout_secs: 30,
        };
        let mut owner = ManagedPrePushCommand::spawn(
            command,
            temporary.path().to_path_buf(),
            Some(JobCancellation::default()),
        );
        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        assert!(Pin::new(&mut owner).poll(&mut context).is_pending());
        let deadline = Instant::now() + Duration::from_secs(2);
        while !entered.exists() {
            assert!(Instant::now() < deadline, "pre-push fixture did not start");
            thread::yield_now();
        }
        let child_pid = std::fs::read_to_string(&pid).expect("read pid");

        drop(owner);

        assert!(
            !Command::new("kill")
                .args(["-0", child_pid.trim()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success()),
            "pre-push owner returned before its child was reaped"
        );
        std::fs::write(&release, b"").expect("release hypothetical survivor");
        assert!(
            !late.exists(),
            "cancelled pre-push mutated the workspace late"
        );
    }
}
