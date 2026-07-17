//! Joined owners for worker-side blocking effects.
//!
//! Skein's blocking pool, like most `spawn_blocking` implementations, cannot
//! cancel a closure after its join future is dropped. Attempt-owned effects use
//! these adapters instead: dropping the async future synchronously cancels a
//! subprocess (when there is one) and joins every owner before control returns
//! to the worker shell's quiescence path.

use std::future::Future;
use std::io::{self, Read};
use std::pin::Pin;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use temper_process_containment::{
    BoundedCapture, CaptureMode, CapturedBytes, CleanupTrigger, ContainmentScope,
};

use crate::executor::{JobCancellation, JobCleanupObserver};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Machine-readable git/fingerprint output is rejected rather than truncated.
pub(crate) const WORKER_COMMAND_COMPLETE_BYTES: usize = 16 * 1024 * 1024;
/// Human-readable command failures retain only a bounded diagnostic tail.
pub(crate) const WORKER_COMMAND_TAIL_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ManagedCommandCapture {
    stdout_mode: CaptureMode,
    stdout_limit: usize,
    stderr_mode: CaptureMode,
    stderr_limit: usize,
}

impl ManagedCommandCapture {
    pub(crate) const fn new(
        stdout_mode: CaptureMode,
        stdout_limit: usize,
        stderr_mode: CaptureMode,
        stderr_limit: usize,
    ) -> Self {
        Self {
            stdout_mode,
            stdout_limit,
            stderr_mode,
            stderr_limit,
        }
    }

    /// Git stdout can contain filenames, hashes, porcelain, patches, or remote
    /// protocol data and therefore must be complete. Stderr is diagnostic.
    pub(crate) const fn git() -> Self {
        Self::new(
            CaptureMode::Complete,
            WORKER_COMMAND_COMPLETE_BYTES,
            CaptureMode::Tail,
            WORKER_COMMAND_TAIL_BYTES,
        )
    }
}

/// Result of a worker-owned command after both streams have been drained and
/// the process containment has been proven empty.
#[derive(Debug)]
pub(crate) struct ManagedCommandOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stderr_dropped_bytes: u64,
}

struct OwnerState<T> {
    result: Option<io::Result<T>>,
    waker: Option<Waker>,
}

/// Runs a blocking closure on a dedicated owner thread that is always joined.
pub(crate) struct JoinedBlocking<T> {
    state: Arc<Mutex<OwnerState<T>>>,
    thread: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> JoinedBlocking<T> {
    pub(crate) fn spawn(
        name: &'static str,
        operation: impl FnOnce() -> T + Send + 'static,
    ) -> Self {
        let state = Arc::new(Mutex::new(OwnerState {
            result: None,
            waker: None,
        }));
        let thread_state = Arc::clone(&state);
        let thread = match thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
                    .map_err(|_| io::Error::other(format!("{name} owner panicked")));
                publish(&thread_state, result);
            }) {
            Ok(thread) => Some(thread),
            Err(error) => {
                publish(
                    &state,
                    Err(io::Error::new(
                        error.kind(),
                        format!("start {name} owner: {error}"),
                    )),
                );
                None
            }
        };
        Self { state, thread }
    }
}

impl<T> JoinedBlocking<T> {
    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl<T> Future for JoinedBlocking<T> {
    type Output = io::Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = take_result(&self.state, cx.waker());
        match result {
            Some(result) => {
                self.join();
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }
}

impl<T> Drop for JoinedBlocking<T> {
    fn drop(&mut self) {
        self.join();
    }
}

/// A contained subprocess whose process tree and waiter/readers are joined on
/// every completion path. Dropping it requests an immediate group kill.
pub(crate) struct ManagedCommand {
    state: Arc<Mutex<OwnerState<ManagedCommandOutput>>>,
    cancelled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ManagedCommand {
    pub(crate) fn spawn(
        command: Command,
        job_cancellation: JobCancellation,
        capture: ManagedCommandCapture,
    ) -> Self {
        let state = Arc::new(Mutex::new(OwnerState {
            result: None,
            waker: None,
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_cancelled = Arc::clone(&cancelled);
        let thread = match thread::Builder::new()
            .name("temper-worker-command".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_command(command, &thread_cancelled, job_cancellation, capture)
                }))
                .unwrap_or_else(|_| Err(io::Error::other("worker command owner panicked")));
                publish(&thread_state, result);
            }) {
            Ok(thread) => Some(thread),
            Err(error) => {
                publish(
                    &state,
                    Err(io::Error::new(
                        error.kind(),
                        format!("start worker command owner: {error}"),
                    )),
                );
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

impl Future for ManagedCommand {
    type Output = io::Result<ManagedCommandOutput>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = take_result(&self.state, cx.waker());
        match result {
            Some(result) => {
                self.join();
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for ManagedCommand {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(thread) = self.thread.as_ref() {
            thread.thread().unpark();
        }
        self.join();
    }
}

fn run_command(
    command: Command,
    cancelled: &AtomicBool,
    job_cancellation: JobCancellation,
    capture: ManagedCommandCapture,
) -> io::Result<ManagedCommandOutput> {
    let owner = std::path::Path::new(command.get_program())
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "worker-command".to_string(), bounded_owner_identifier);
    let contained_command = crate::process_containment::containment_command(
        &command,
        Stdio::null(),
        Stdio::piped(),
        Stdio::piped(),
    );
    let prepared = crate::process_containment::prepare_with_observer(
        "worker-command",
        "local",
        ContainmentScope::WorkerCommand,
        owner.as_str(),
        Some(Arc::new(JobCleanupObserver(job_cancellation))),
    )?;
    let process = prepared.spawn(contained_command)?;

    let stdout = match process.take_stdout()? {
        Some(stdout) => stdout,
        None => {
            let _ = process.cleanup(CleanupTrigger::Shutdown);
            return Err(io::Error::other("worker command stdout was not piped"));
        }
    };
    let stderr = match process.take_stderr()? {
        Some(stderr) => stderr,
        None => {
            let _ = process.cleanup(CleanupTrigger::Shutdown);
            return Err(io::Error::other("worker command stderr was not piped"));
        }
    };
    let stdout_reader =
        match spawn_reader("stdout", stdout, capture.stdout_mode, capture.stdout_limit) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = process.cleanup(CleanupTrigger::Shutdown);
                return Err(error);
            }
        };
    let stderr_reader =
        match spawn_reader("stderr", stderr, capture.stderr_mode, capture.stderr_limit) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = process.cleanup(CleanupTrigger::Shutdown);
                let _ = stdout_reader.join();
                return Err(error);
            }
        };

    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = process.cleanup(CleanupTrigger::Cancellation);
            let _ = join_reader(stdout_reader, "stdout");
            let _ = join_reader(stderr_reader, "stderr");
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "worker command cancelled after proven cleanup",
            ));
        }
        match process.try_wait_root() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::park_timeout(PROCESS_POLL_INTERVAL),
            Err(error) => {
                let _ = process.cleanup(CleanupTrigger::Shutdown);
                let _ = join_reader(stdout_reader, "stdout");
                let _ = join_reader(stderr_reader, "stderr");
                return Err(error);
            }
        }
    };

    // Git and configured helpers can background descendants which retain the
    // output pipes after the direct child exits. Cleanup proves recursive
    // emptiness before joining readers or publishing command completion.
    let _cleanup = process.cleanup(CleanupTrigger::NormalRootExit);
    let stdout = join_reader(stdout_reader, "stdout")?;
    let stderr = join_reader(stderr_reader, "stderr")?;
    Ok(ManagedCommandOutput {
        status,
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
        stderr_dropped_bytes: stderr.dropped_bytes(),
    })
}

fn bounded_owner_identifier(value: &str) -> String {
    let mut value = value.to_string();
    let limit = temper_process_containment::MAX_CONTAINMENT_IDENTITY_BYTES;
    if value.len() > limit {
        let mut end = limit;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    if value.is_empty() {
        "worker-command".to_string()
    } else {
        value
    }
}

fn spawn_reader(
    stream: &'static str,
    mut reader: impl Read + Send + 'static,
    mode: CaptureMode,
    limit: usize,
) -> io::Result<JoinHandle<io::Result<CapturedBytes>>> {
    thread::Builder::new()
        .name(format!("temper-worker-command-{stream}"))
        .spawn(move || {
            let mut capture = BoundedCapture::new(mode, limit);
            capture.drain(&mut reader)?;
            capture
                .finish()
                .map_err(|overflow| io::Error::new(io::ErrorKind::FileTooLarge, overflow))
        })
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("start worker command {stream} reader: {error}"),
            )
        })
}

fn join_reader(
    reader: JoinHandle<io::Result<CapturedBytes>>,
    stream: &str,
) -> io::Result<CapturedBytes> {
    reader
        .join()
        .map_err(|_| io::Error::other(format!("worker command {stream} reader panicked")))?
}

fn publish<T>(state: &Arc<Mutex<OwnerState<T>>>, result: io::Result<T>) {
    let waker = {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.result = Some(result);
        state.waker.take()
    };
    if let Some(waker) = waker {
        waker.wake();
    }
}

fn take_result<T>(
    state: &Arc<Mutex<OwnerState<T>>>,
    current_waker: &Waker,
) -> Option<io::Result<T>> {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.result.is_none()
        && !state
            .waker
            .as_ref()
            .is_some_and(|waker| waker.will_wake(current_waker))
    {
        state.waker = Some(current_waker.clone());
    }
    state.result.take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::task::{Wake, Waker};
    use std::time::{Duration, Instant};

    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    #[test]
    #[cfg(unix)]
    fn complete_capture_fails_after_draining_overflow() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf abcdef"]);
        let error = run_command(
            command,
            &AtomicBool::new(false),
            JobCancellation::default(),
            ManagedCommandCapture::new(CaptureMode::Complete, 4, CaptureMode::Tail, 4),
        )
        .expect_err("partial machine output must not escape");

        assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
        assert!(error.to_string().contains("observing 6 bytes"), "{error}");
    }

    #[test]
    #[cfg(unix)]
    fn tail_capture_reports_dropped_diagnostics() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf abcdef >&2"]);
        let output = run_command(
            command,
            &AtomicBool::new(false),
            JobCancellation::default(),
            ManagedCommandCapture::new(CaptureMode::Complete, 4, CaptureMode::Tail, 4),
        )
        .expect("tail capture");

        assert_eq!(output.stderr, b"cdef");
        assert_eq!(output.stderr_dropped_bytes, 2);
    }

    #[test]
    #[cfg(unix)]
    fn dropping_command_kills_and_joins_before_late_mutation() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("tempdir");
        let script = temp.path().join("paused-effect.sh");
        let pid = temp.path().join("pid");
        let entered = temp.path().join("entered");
        let release = temp.path().join("release");
        let late_mutation = temp.path().join("late-mutation");
        std::fs::write(
            &script,
            "#!/bin/sh\nset -eu\nprintf '%s' \"$$\" > \"$1\"\ntouch \"$2\"\nwhile [ ! -e \"$3\" ]; do sleep 0.01; done\nprintf late > \"$4\"\n",
        )
        .expect("write script");
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();

        let mut process = Command::new(&script);
        process.args([&pid, &entered, &release, &late_mutation]);
        let mut effect = ManagedCommand::spawn(
            process,
            JobCancellation::default(),
            ManagedCommandCapture::git(),
        );
        let waker = Waker::from(Arc::new(NoopWake));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut effect).poll(&mut cx).is_pending());
        wait_for_path(&entered);
        let child_pid = std::fs::read_to_string(&pid).expect("child pid");

        drop(effect);

        assert!(
            !process_exists(&child_pid),
            "managed command returned from Drop before its child was reaped"
        );
        std::fs::write(&release, b"").expect("release hypothetical detached command");
        assert!(
            !late_mutation.exists(),
            "cancelled command mutated state after its owner joined"
        );
    }

    #[cfg(unix)]
    fn process_exists(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn wait_for_path(path: &std::path::Path) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {}",
                path.display()
            );
            thread::yield_now();
        }
    }
}
