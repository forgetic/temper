//! A throwaway, host-mode `forgejo-runner` for the ignored CI fixture.
//!
//! [`ForgejoRunner::register`] takes a running [`ForgejoServer`], obtains a
//! runner registration token via the server CLI, registers a **host-mode**
//! runner (`--labels host:host`, **no containers**) in a fresh temp dir, and
//! spawns `forgejo-runner daemon` behind a kill-on-drop guard. The daemon
//! process is killed and the temp dir removed on drop, so a panicking test
//! never orphans a runner or leaks its working dir.
//!
//! Like [`ForgejoServer`], this is **never** reached by the default test suite:
//! only `#[ignore]`d tests construct one. CI is real here — the runner executes
//! genuine jobs on this host. On first startup, the pinned runner binary is
//! downloaded and published to the shared cache through the same process-safe
//! cache path as the server when no explicit override or cached binary exists.

use super::download;
use super::{ForgejoServer, ServerError};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// A failure registering or operating the throwaway runner.
#[derive(Debug)]
pub enum RunnerError {
    /// Resolving the `forgejo-runner` binary failed.
    Binary(download::DownloadError),
    /// A filesystem operation on the runner dir failed.
    Io(std::io::Error),
    /// Obtaining a registration token from the server failed.
    Token(ServerError),
    /// `forgejo-runner register` exited non-zero.
    Register(String),
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunnerError::Binary(err) => write!(f, "{err}"),
            RunnerError::Io(err) => write!(f, "forgejo-runner io error: {err}"),
            RunnerError::Token(err) => write!(f, "runner registration token failed: {err}"),
            RunnerError::Register(output) => {
                write!(f, "`forgejo-runner register` failed: {output}")
            }
        }
    }
}

impl std::error::Error for RunnerError {}

impl From<std::io::Error> for RunnerError {
    fn from(err: std::io::Error) -> Self {
        RunnerError::Io(err)
    }
}
impl From<download::DownloadError> for RunnerError {
    fn from(err: download::DownloadError) -> Self {
        RunnerError::Binary(err)
    }
}

static NEXT_RUNNER: AtomicU64 = AtomicU64::new(0);

/// A registered, running host-mode `forgejo-runner`. Killed and cleaned up on
/// drop.
pub struct ForgejoRunner {
    binary: PathBuf,
    work_dir: PathBuf,
    name: String,
    daemon: Child,
}

impl ForgejoRunner {
    /// The host-mode label this runner registers with (`host:host`). A workflow
    /// must declare `runs-on: host` to be picked up — and no Docker is used.
    pub const HOST_LABEL: &'static str = "host:host";

    /// Registers a host-mode runner against `server` and spawns its daemon.
    ///
    /// Obtains a registration token from the server CLI
    /// (`forgejo actions generate-runner-token`), runs `forgejo-runner register
    /// --no-interactive --labels host:host` in a fresh temp dir (writing a
    /// `.runner` file there), then spawns `forgejo-runner daemon` from that dir
    /// behind a kill-on-drop guard.
    pub fn register(server: &ForgejoServer) -> Result<Self, RunnerError> {
        let binary = download::ensure_runner_binary()?;
        let identity = next_runner_identity();
        let work_dir = identity.work_dir;
        let name = identity.name;
        let _ = std::fs::remove_dir_all(&work_dir);
        std::fs::create_dir_all(&work_dir)?;

        let token = registration_token(server).map_err(RunnerError::Token)?;
        register_runner(&binary, &work_dir, server.base_url(), &token, &name)?;
        let daemon = spawn_daemon(&binary, &work_dir)?;

        Ok(Self {
            binary,
            work_dir,
            name,
            daemon,
        })
    }

    /// The runner's registered name (unique per instance).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The resolved `forgejo-runner` binary path.
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// The runner's working dir (holds `.runner` and `daemon.log`).
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Whether the daemon process is still running. `false` once it has exited
    /// (e.g. it crashed); useful for a readiness/liveness check in tests.
    pub fn is_running(&mut self) -> bool {
        matches!(self.daemon.try_wait(), Ok(None))
    }

    /// The last few lines of the daemon log, for diagnostics on failure.
    pub fn log_tail(&self) -> String {
        std::fs::read_to_string(self.work_dir.join("daemon.log"))
            .map(|log| {
                log.lines()
                    .rev()
                    .take(8)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_default()
    }
}

impl Drop for ForgejoRunner {
    fn drop(&mut self) {
        // Kill the daemon, then remove the temp work dir. Best-effort: a
        // panicking test must never orphan a runner or leak its working dir.
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_dir_all(&self.work_dir);
    }
}

#[derive(Debug)]
struct RunnerIdentity {
    name: String,
    work_dir: PathBuf,
}

fn next_runner_identity() -> RunnerIdentity {
    let id = NEXT_RUNNER.fetch_add(1, Ordering::SeqCst);
    RunnerIdentity {
        name: format!("temper-runner-{}-{id}", std::process::id()),
        work_dir: runner_dir_for_id(id),
    }
}

fn runner_dir_for_id(id: u64) -> PathBuf {
    std::env::temp_dir().join(format!("temper-forgejo-runner-{}-{id}", std::process::id()))
}

/// Obtains a runner registration token from the server CLI. The CLI path needs
/// no admin token (the admin-token REST path is the Phase 2 alternative).
fn registration_token(server: &ForgejoServer) -> Result<String, ServerError> {
    let token = server.run_cli(&["actions", "generate-runner-token"])?;
    Ok(token.trim().to_string())
}

fn register_runner(
    binary: &Path,
    work_dir: &Path,
    instance: &str,
    token: &str,
    name: &str,
) -> Result<(), RunnerError> {
    // `register` writes `.runner` into the current working directory.
    let output = Command::new(binary)
        .current_dir(work_dir)
        .args([
            "register",
            "--no-interactive",
            "--instance",
            instance,
            "--token",
            token,
            "--name",
            name,
            "--labels",
            ForgejoRunner::HOST_LABEL,
        ])
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        let mut combined = String::from_utf8_lossy(&output.stderr).to_string();
        combined.push_str(&String::from_utf8_lossy(&output.stdout));
        Err(RunnerError::Register(combined.trim().to_string()))
    }
}

fn spawn_daemon(binary: &Path, work_dir: &Path) -> Result<Child, RunnerError> {
    // `daemon` reads `.runner` from its working dir; log to a file so failures
    // are diagnosable without inheriting the test's stdio.
    let log = std::fs::File::create(work_dir.join("daemon.log"))?;
    let mut command = Command::new(binary);
    command
        .current_dir(work_dir)
        .arg("daemon")
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    // Bound the runner (also a Go program) to the same CPU cap as the server so
    // real CI jobs cannot saturate the host either (see `super::apply_cpu_cap`).
    super::apply_cpu_cap(&mut command);
    let child = command.spawn()?;
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier, Mutex};

    #[test]
    fn runner_identities_are_unique_under_parallel_generation() {
        let threads = 16;
        let per_thread = 16;
        let barrier = Arc::new(Barrier::new(threads));
        let identities = Arc::new(Mutex::new(Vec::new()));
        let handles = (0..threads)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let identities = Arc::clone(&identities);
                std::thread::spawn(move || {
                    barrier.wait();
                    let mut local = Vec::new();
                    for _ in 0..per_thread {
                        let identity = next_runner_identity();
                        local.push((identity.name, identity.work_dir));
                    }
                    identities.lock().unwrap().extend(local);
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().expect("identity thread panicked");
        }
        let identities = identities.lock().unwrap();
        let names = identities
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<HashSet<_>>();
        let work_dirs = identities
            .iter()
            .map(|(_, work_dir)| work_dir.clone())
            .collect::<HashSet<_>>();
        assert_eq!(names.len(), threads * per_thread);
        assert_eq!(work_dirs.len(), threads * per_thread);
    }
}
