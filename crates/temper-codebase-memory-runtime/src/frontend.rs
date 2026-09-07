use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{LaunchConfig, ProviderBootstrapFailure};

const MAX_INITIALIZE_RECORD: usize = 65_536;
const FRONTEND_CLOSE_GRACE: Duration = Duration::from_secs(2);

/// Private bootstrap-only frontend. Its enclosing helper/containment owns ALL
/// descendants; closing this session never triggers recursive daemon cleanup.
pub(crate) struct BootstrapFrontend {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
    response: mpsc::Receiver<Result<Value, ProviderBootstrapFailure>>,
}

impl BootstrapFrontend {
    pub(crate) fn spawn(config: &LaunchConfig) -> Result<Self, ProviderBootstrapFailure> {
        let (stdout, child_stdout) =
            UnixStream::pair().map_err(|_| ProviderBootstrapFailure::Spawn)?;
        stdout
            .set_read_timeout(Some(Duration::from_millis(50)))
            .map_err(|_| ProviderBootstrapFailure::Transport)?;
        let stopping = Arc::new(AtomicBool::new(false));
        let reader_stopping = Arc::clone(&stopping);
        let mut child = Command::new(&config.command)
            .args(config.bootstrap_args())
            .envs(config.environment.iter().map(|(key, value)| (key, value)))
            .current_dir(&config.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(std::os::fd::OwnedFd::from(child_stdout)))
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| ProviderBootstrapFailure::Spawn)?;
        let stdin = child.stdin.take();
        let (sender, response) = mpsc::sync_channel(1);
        let reader = thread::Builder::new()
            .name("cbm-bootstrap-reader".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let result = read_initialize(&mut reader, &reader_stopping);
                let _ = sender.send(result);
                // Keep notifications from filling the frontend pipe. Nothing from
                // this bootstrap-only connection is exposed to the model or logs.
                let mut bytes = [0; 4096];
                while !reader_stopping.load(Ordering::Acquire) {
                    match reader.read(&mut bytes) {
                        Ok(0) => break,
                        Err(error) if !retry_read(&error) => break,
                        _ => {}
                    }
                }
            });
        let reader = match reader {
            Ok(reader) => reader,
            Err(_) => {
                drop(stdin);
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProviderBootstrapFailure::Spawn);
            }
        };
        Ok(Self {
            child,
            stdin,
            reader: Some(reader),
            stopping,
            response,
        })
    }

    pub(crate) fn initialize(&mut self, timeout: Duration) -> Result<(), ProviderBootstrapFailure> {
        self.write(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"temper-bootstrap","version":"1"}
        }}))?;
        let response = self
            .response
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => ProviderBootstrapFailure::Timeout,
                mpsc::RecvTimeoutError::Disconnected => ProviderBootstrapFailure::Transport,
            })??;
        let server = &response["result"]["serverInfo"];
        if response["id"] != 1 || server["name"] != "codebase-memory-mcp" {
            return Err(ProviderBootstrapFailure::ProviderContract);
        }
        self.write(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
    }

    fn write(&mut self, value: Value) -> Result<(), ProviderBootstrapFailure> {
        let mut bytes = value.to_string().into_bytes();
        bytes.push(b'\n');
        self.stdin
            .as_mut()
            .ok_or(ProviderBootstrapFailure::Transport)?
            .write_all(&bytes)
            .map_err(|_| ProviderBootstrapFailure::Transport)
    }
}

impl Drop for BootstrapFrontend {
    fn drop(&mut self) {
        self.stdin.take();
        let started = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                _ if started.elapsed() >= FRONTEND_CLOSE_GRACE => {
                    // Exact direct frontend only. Its daemon descendants stay
                    // owned by the enclosing subreaper until native shutdown.
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
                _ => thread::sleep(Duration::from_millis(10)),
            }
        }
        self.stopping.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn read_initialize(
    reader: &mut impl BufRead,
    stopping: &AtomicBool,
) -> Result<Value, ProviderBootstrapFailure> {
    for _ in 0..32 {
        let mut bytes = Vec::new();
        while bytes.last() != Some(&b'\n') {
            if stopping.load(Ordering::Acquire) {
                return Err(ProviderBootstrapFailure::Transport);
            }
            match reader
                .take((MAX_INITIALIZE_RECORD + 1 - bytes.len()) as u64)
                .read_until(b'\n', &mut bytes)
            {
                Ok(0) => return Err(ProviderBootstrapFailure::Transport),
                Err(error) if retry_read(&error) => continue,
                Err(_) => return Err(ProviderBootstrapFailure::Transport),
                Ok(_) => {}
            }
            if bytes.len() > MAX_INITIALIZE_RECORD {
                return Err(ProviderBootstrapFailure::ProviderContract);
            }
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderBootstrapFailure::ProviderContract)?;
        if value.get("id").is_some() {
            return Ok(value);
        }
    }
    Err(ProviderBootstrapFailure::ProviderContract)
}

fn retry_read(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::Interrupted
    )
}
