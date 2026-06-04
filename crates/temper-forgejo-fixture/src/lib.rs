//! Shared throwaway Forgejo fixture for ignored, local-only tests.
//!
//! [`ForgejoServer::start`] boots a real Forgejo from a resolved binary — first
//! a `TEMPER_FORGEJO_BINARY` override, then the gitignored `.cache/forgejo/`
//! cache, then a pinned checked download published through a process-safe cache
//! lock — against a fresh SQLite data dir on an ephemeral port, waits for
//! `/api/v1/version`, and kills the process plus removes the data dir on drop.
//! [`ForgejoServer::start_with_state`] instead restores a JSON-declared cached
//! data tree from `.cache/forgejo/states/` into a unique `/tmp` directory before
//! starting the same per-test process. [`ForgejoRunner::register`] does the same
//! process lifecycle for a host-mode `forgejo-runner`.
//!
//! Default non-ignored tests stay offline because they never construct these
//! real process fixtures; ignored/local Forgejo tests download the pinned assets
//! automatically on first startup and reuse `.cache/forgejo/` afterward.

pub mod download;
pub mod runner;
pub mod state;

pub use runner::{ForgejoRunner, RunnerError};
pub use state::{CachedForgejo, ForgejoState};

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long to wait for `forgejo web` to answer before giving up.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Poll interval while waiting for readiness.
const READY_POLL: Duration = Duration::from_millis(200);
/// Narrow retry count for the racy free-port → Go bind handoff.
const WEB_BIND_RETRY_ATTEMPTS: usize = 3;

/// A failure starting or operating the throwaway server.
#[derive(Debug)]
pub enum ServerError {
    /// Resolving the Forgejo binary failed.
    Binary(download::DownloadError),
    /// A filesystem operation on the data dir failed.
    Io(std::io::Error),
    /// A `forgejo` subcommand exited non-zero.
    Command { command: String, output: String },
    /// The server never answered within [`READY_TIMEOUT`].
    NotReady(String),
    /// A cached-state description or metadata file was not valid JSON.
    Json(serde_json::Error),
    /// A cached-state initializer failed while materializing the state.
    Initialize(String),
    /// Cache coordination or publication failed.
    Cache(String),
    /// A clean shutdown request failed or timed out.
    Shutdown(String),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerError::Binary(err) => write!(f, "{err}"),
            ServerError::Io(err) => write!(f, "forgejo server io error: {err}"),
            ServerError::Command { command, output } => {
                write!(f, "`forgejo {command}` failed: {output}")
            }
            ServerError::NotReady(why) => write!(f, "forgejo never became ready: {why}"),
            ServerError::Json(err) => write!(f, "forgejo cached-state json error: {err}"),
            ServerError::Initialize(why) => write!(f, "forgejo cached-state init failed: {why}"),
            ServerError::Cache(why) => write!(f, "forgejo cached-state cache error: {why}"),
            ServerError::Shutdown(why) => write!(f, "forgejo clean shutdown failed: {why}"),
        }
    }
}

impl std::error::Error for ServerError {}

impl From<std::io::Error> for ServerError {
    fn from(err: std::io::Error) -> Self {
        ServerError::Io(err)
    }
}
impl From<download::DownloadError> for ServerError {
    fn from(err: download::DownloadError) -> Self {
        ServerError::Binary(err)
    }
}
impl From<serde_json::Error> for ServerError {
    fn from(err: serde_json::Error) -> Self {
        ServerError::Json(err)
    }
}

static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(0);

/// A running throwaway Forgejo instance. Killed and cleaned up on drop.
pub struct ForgejoServer {
    binary: PathBuf,
    data_dir: PathBuf,
    config_path: PathBuf,
    base_url: String,
    child: Child,
}

impl ForgejoServer {
    /// Boots a fresh instance: writes config, migrates, spawns `web`, and waits
    /// for readiness. The returned handle owns the process and data dir.
    pub fn start() -> Result<Self, ServerError> {
        let binary = download::ensure_binary()?;
        let data_dir = unique_data_dir();
        let _ = std::fs::remove_dir_all(&data_dir);
        for sub in ["custom/conf", "data", "log", "repos"] {
            std::fs::create_dir_all(data_dir.join(sub))?;
        }

        let (config_path, base_url) = write_runtime_config(&data_dir)?;

        // `migrate` initializes the SQLite schema before the web server starts.
        run_forgejo(&binary, &config_path, &["migrate"])?;

        Self::spawn_prepared(binary, data_dir, config_path, base_url)
    }

    /// Boots against an already-prepared Forgejo data tree. The caller owns the
    /// tree contents; this rewrites only `custom/conf/app.ini` with this run's
    /// temp path and port, then starts `forgejo web` without re-running migrate.
    pub(crate) fn start_from_prepared_dir(data_dir: PathBuf) -> Result<Self, ServerError> {
        let binary = download::ensure_binary()?;
        let (config_path, base_url) = write_runtime_config(&data_dir)?;
        Self::spawn_prepared(binary, data_dir, config_path, base_url)
    }

    fn spawn_prepared(
        binary: PathBuf,
        data_dir: PathBuf,
        mut config_path: PathBuf,
        mut base_url: String,
    ) -> Result<Self, ServerError> {
        for attempt in 0..WEB_BIND_RETRY_ATTEMPTS {
            let mut child = spawn_web(&binary, &config_path, &data_dir)?;
            match wait_until_ready(&mut child, &base_url, &data_dir) {
                Ok(()) => {
                    return Ok(Self {
                        binary,
                        data_dir,
                        config_path,
                        base_url,
                        child,
                    });
                }
                Err(err)
                    if attempt + 1 < WEB_BIND_RETRY_ATTEMPTS && is_address_in_use_startup(&err) =>
                {
                    let _ = child.kill();
                    let _ = child.wait();
                    let (next_config_path, next_base_url) = write_runtime_config(&data_dir)?;
                    config_path = next_config_path;
                    base_url = next_base_url;
                }
                Err(err) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err);
                }
            }
        }
        unreachable!("web bind retry loop always returns from an attempt")
    }

    /// The base URL (`http://127.0.0.1:<port>`), no trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The active config file path (used by `forgejo` admin subcommands).
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// The instance data directory (holds `web.log`, the SQLite db, repos).
    /// Exposed for diagnostics in the ignored e2e tests.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The resolved server binary path.
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// Runs a `forgejo` admin/CLI subcommand against this instance's config,
    /// returning trimmed stdout. Used by later phases for admin bootstrap.
    pub fn run_cli(&self, args: &[&str]) -> Result<String, ServerError> {
        run_forgejo(&self.binary, &self.config_path, args)
    }

    /// Requests a graceful web-server stop and waits for the process to exit.
    /// Used only while publishing reusable cached state: normal test teardown
    /// intentionally uses `Child::kill` in [`Drop`] so every test shuts down
    /// quickly by SIGKILL/TerminateProcess.
    pub(crate) fn stop_cleanly(&mut self) -> Result<(), ServerError> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }
        request_graceful_stop(&mut self.child)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(ServerError::Shutdown(format!(
                    "process did not exit within 15s after graceful stop; log: {}",
                    self.read_log_tail()
                )));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn read_log_tail(&self) -> String {
        read_log_tail(&self.data_dir)
    }
}

fn wait_until_ready(child: &mut Child, base_url: &str, data_dir: &Path) -> Result<(), ServerError> {
    let version_url = format!("{base_url}/api/v1/version");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|err| ServerError::NotReady(err.to_string()))?;
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        // Surface an early crash instead of polling a dead process.
        if let Some(status) = child.try_wait()? {
            return Err(ServerError::NotReady(format!(
                "process exited early with {status}; log: {}",
                read_log_tail(data_dir)
            )));
        }
        if let Ok(response) = client.get(&version_url).send() {
            if response.status().is_success() {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(ServerError::NotReady(format!(
                "no 200 from {version_url} within {READY_TIMEOUT:?}; log: {}",
                read_log_tail(data_dir)
            )));
        }
        std::thread::sleep(READY_POLL);
    }
}

fn read_log_tail(data_dir: &Path) -> String {
    std::fs::read_to_string(data_dir.join("web.log"))
        .map(|log| {
            log.lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .unwrap_or_default()
}

fn is_address_in_use_startup(err: &ServerError) -> bool {
    match err {
        ServerError::NotReady(why) => why.to_ascii_lowercase().contains("address already in use"),
        _ => false,
    }
}

impl Drop for ForgejoServer {
    fn drop(&mut self) {
        // Kill the web server, then remove the temp data dir. Best-effort: a
        // panicking test must never orphan a process or leak a data dir.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

pub(crate) fn unique_data_dir() -> PathBuf {
    let id = NEXT_INSTANCE.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("temper-forgejo-{}-{id}", std::process::id()))
}

/// Binds `127.0.0.1:0`, reads the assigned port, then releases it. There is an
/// unavoidable race between release and the server's later Go-side bind, so
/// startup retries only the clear address-in-use failure this handoff can cause.
fn free_port() -> Result<u16, ServerError> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Default cap on the number of OS threads the spawned Forgejo (a Go program)
/// uses to run goroutines, bounding its CPU to roughly this many cores.
///
/// The throwaway e2e Forgejo otherwise drives its host to **2+ cores of
/// *sustained* CPU for minutes** under the multi-process workload (an
/// actions/git/indexer load characteristic of this build, observed while the
/// real-agent e2e ran): not a brief spike, but a steady runaway that can
/// saturate a small dev box for the whole run. Capping `GOMAXPROCS` bounds it at
/// the source so the rest of the run (cargo, the worker children, agent IO)
/// always keeps cores free, regardless of run length.
///
/// `taskset` alone is insufficient: `taskset -cp` re-pins only a process's main
/// thread, and Go spreads goroutines across `GOMAXPROCS` OS threads that keep
/// running on every core. `GOMAXPROCS` caps the thread count itself.
///
/// Override with `TEMPER_FORGEJO_GOMAXPROCS` (set it to the empty string to
/// leave Go's default — one thread per core — in place).
const DEFAULT_FORGEJO_GOMAXPROCS: &str = "2";

/// The `GOMAXPROCS` value to set on spawned Forgejo processes, or `None` to leave
/// it unset (explicit opt-out via an empty `TEMPER_FORGEJO_GOMAXPROCS`).
fn forgejo_gomaxprocs() -> Option<String> {
    match std::env::var("TEMPER_FORGEJO_GOMAXPROCS") {
        Ok(value) if value.trim().is_empty() => None,
        Ok(value) => Some(value.trim().to_string()),
        Err(_) => Some(DEFAULT_FORGEJO_GOMAXPROCS.to_string()),
    }
}

/// Applies the [`forgejo_gomaxprocs`] CPU cap to `command` (a Forgejo or
/// `forgejo-runner` invocation). Child processes Forgejo spawns (git hooks)
/// inherit the env, so the cap propagates to them too.
pub(crate) fn apply_cpu_cap(command: &mut Command) {
    if let Some(value) = forgejo_gomaxprocs() {
        command.env("GOMAXPROCS", value);
    }
}

fn write_runtime_config(data_dir: &Path) -> Result<(PathBuf, String), ServerError> {
    let port = free_port()?;
    let base_url = format!("http://127.0.0.1:{port}");
    let config_path = data_dir.join("custom/conf/app.ini");
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config_path, app_ini(data_dir, port, &base_url))?;
    Ok((config_path, base_url))
}

fn request_graceful_stop(child: &mut Child) -> Result<(), ServerError> {
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(ServerError::Shutdown(format!(
                "`kill -TERM {}` exited with {status}",
                child.id()
            )))
        }
    }
    #[cfg(not(unix))]
    {
        child.kill()?;
        Ok(())
    }
}

fn run_forgejo(binary: &Path, config: &Path, args: &[&str]) -> Result<String, ServerError> {
    let mut command = Command::new(binary);
    command.arg("--config").arg(config).args(args).env(
        "GITEA_WORK_DIR",
        config
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .unwrap_or(config),
    );
    apply_cpu_cap(&mut command);
    let output = command.output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let mut combined = String::from_utf8_lossy(&output.stderr).to_string();
        combined.push_str(&String::from_utf8_lossy(&output.stdout));
        Err(ServerError::Command {
            command: args.join(" "),
            output: combined.trim().to_string(),
        })
    }
}

fn spawn_web(binary: &Path, config: &Path, data_dir: &Path) -> Result<Child, ServerError> {
    use std::process::Stdio;
    let log = std::fs::File::create(data_dir.join("web.log"))?;
    let mut command = Command::new(binary);
    command
        .arg("--config")
        .arg(config)
        .arg("web")
        .env("GITEA_WORK_DIR", data_dir)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    apply_cpu_cap(&mut command);
    let child = command.spawn()?;
    Ok(child)
}

fn app_ini(data_dir: &Path, port: u16, base_url: &str) -> String {
    let root = data_dir.display();
    // A minimal hermetic config: SQLite, no SSH, no mailer, no registration,
    // install lock set so the web server starts straight into the app.
    format!(
        "APP_NAME = Temper Forgejo E2E\n\
         RUN_MODE = prod\n\
         WORK_PATH = {root}\n\
         \n\
         [server]\n\
         PROTOCOL = http\n\
         HTTP_ADDR = 127.0.0.1\n\
         HTTP_PORT = {port}\n\
         ROOT_URL = {base_url}/\n\
         DISABLE_SSH = true\n\
         START_SSH_SERVER = false\n\
         OFFLINE_MODE = true\n\
         APP_DATA_PATH = {root}/data\n\
         \n\
         [database]\n\
         DB_TYPE = sqlite3\n\
         PATH = {root}/data/forgejo.db\n\
         LOG_SQL = false\n\
         \n\
         [repository]\n\
         ROOT = {root}/repos\n\
         \n\
         [log]\n\
         ROOT_PATH = {root}/log\n\
         MODE = console\n\
         LEVEL = error\n\
         \n\
         [security]\n\
         INSTALL_LOCK = true\n\
         SECRET_KEY = temper-e2e-secret-not-for-production\n\
         INTERNAL_TOKEN = temper-e2e-internal-token-not-for-production\n\
         \n\
         [service]\n\
         DISABLE_REGISTRATION = true\n\
         REQUIRE_SIGNIN_VIEW = false\n\
         \n\
         [mailer]\n\
         ENABLED = false\n\
         \n\
         [webhook]\n\
         ALLOWED_HOST_LIST = 127.0.0.1,localhost\n\
         \n\
         [actions]\n\
         ENABLED = true\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_port_is_nonzero_and_distinct() {
        let a = free_port().expect("port a");
        let b = free_port().expect("port b");
        assert_ne!(a, 0);
        assert_ne!(b, 0);
    }

    #[test]
    fn cpu_cap_defaults_to_two_and_is_applied() {
        // Documents the GOMAXPROCS mitigation for the sustained-CPU incident.
        assert_eq!(DEFAULT_FORGEJO_GOMAXPROCS, "2");
        let mut command = Command::new("forgejo");
        apply_cpu_cap(&mut command);
        let gomaxprocs = command
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new("GOMAXPROCS"))
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned());
        // Only assert the default path so the test is not racy against an
        // override; in the default test env the var is unset.
        if std::env::var("TEMPER_FORGEJO_GOMAXPROCS").is_err() {
            assert_eq!(gomaxprocs.as_deref(), Some("2"));
        }
    }

    #[test]
    fn data_dirs_are_unique_under_parallel_generation() {
        let threads = 16;
        let per_thread = 16;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(threads));
        let dirs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let handles = (0..threads)
            .map(|_| {
                let barrier = std::sync::Arc::clone(&barrier);
                let dirs = std::sync::Arc::clone(&dirs);
                std::thread::spawn(move || {
                    barrier.wait();
                    let local = (0..per_thread)
                        .map(|_| unique_data_dir())
                        .collect::<Vec<_>>();
                    dirs.lock().unwrap().extend(local);
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().expect("data-dir thread panicked");
        }
        let dirs = dirs.lock().unwrap();
        let unique = dirs
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), threads * per_thread);
    }

    #[test]
    fn address_in_use_retry_detector_is_narrow() {
        assert!(is_address_in_use_startup(&ServerError::NotReady(
            "process exited early; log: listen tcp 127.0.0.1:3000: bind: address already in use"
                .to_string()
        )));
        assert!(!is_address_in_use_startup(&ServerError::NotReady(
            "no 200 from version endpoint within timeout".to_string()
        )));
        assert!(!is_address_in_use_startup(&ServerError::Command {
            command: "migrate".to_string(),
            output: "address already in use in unrelated output".to_string(),
        }));
    }

    #[test]
    fn app_ini_sets_port_and_sqlite() {
        let ini = app_ini(Path::new("/tmp/x"), 4321, "http://127.0.0.1:4321");
        assert!(ini.contains("HTTP_PORT = 4321"));
        assert!(ini.contains("DB_TYPE = sqlite3"));
        assert!(ini.contains("INSTALL_LOCK = true"));
        // Actions must be enabled so a host-mode forgejo-runner has work to run.
        assert!(ini.contains("[actions]"));
        assert!(ini.contains("ENABLED = true"));
    }
}
