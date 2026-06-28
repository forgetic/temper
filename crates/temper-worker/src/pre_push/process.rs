use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::PrePushCommand;

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

pub(super) async fn run_command(command: PrePushCommand, cwd: PathBuf) -> PrePushCommandResult {
    skein::runtime::spawn_blocking(move || run_command_sync(command, cwd)).await
}

fn run_command_sync(command: PrePushCommand, cwd: PathBuf) -> PrePushCommandResult {
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
    configure_command(&mut process);
    let mut child = match process
        .args(args)
        .current_dir(&result.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            result.error = Some(format!("spawn: {error}"));
            result.elapsed_ms = elapsed_ms(started);
            return result;
        }
    };

    let stdout = child.stdout.take().expect("stdout was configured as piped");
    let stderr = child.stderr.take().expect("stderr was configured as piped");
    let stdout_reader = spawn_reader(stdout);
    let stderr_reader = spawn_reader(stderr);

    match wait_with_timeout(&mut child, Duration::from_secs(result.timeout_secs)) {
        Ok(outcome) => {
            result.timed_out = outcome.timed_out;
            if let Some(status) = outcome.status {
                result.exit_code = status.code();
                result.exit_status = Some(status.to_string());
            }
        }
        Err(error) => {
            result.error = Some(format!("wait: {error}"));
            let _ = child.kill();
            let _ = child.wait();
        }
    }

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
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> io::Result<WaitOutcome> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(WaitOutcome {
                status: Some(status),
                timed_out: false,
            });
        }
        if started.elapsed() >= timeout {
            kill_for_timeout(child)?;
            return Ok(WaitOutcome {
                status: child.wait().ok(),
                timed_out: true,
            });
        }
        thread::sleep(timeout.saturating_sub(started.elapsed()).min(POLL_INTERVAL));
    }
}

fn configure_command(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Put each check in its own process group so a timeout can terminate
        // grandchildren (for example, a shell's `sleep`) before stdout/stderr
        // reader threads wait for inherited pipe handles to close.
        command.process_group(0);
    }
}

fn kill_for_timeout(child: &mut Child) -> io::Result<()> {
    kill_process_group(child);
    match child.kill() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(()),
        Err(error) => Err(error),
    }
}

fn kill_process_group(child: &Child) {
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg("--")
            .arg(&group)
            .status();
    }
}

fn spawn_reader<R>(mut reader: R) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || read_tail(&mut reader, OUTPUT_TAIL_BYTES))
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
