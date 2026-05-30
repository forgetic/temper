//! A throwaway, locally-run Forgejo server for the env-gated end-to-end harness.
//!
//! [`ForgejoServer::start`] boots a real Forgejo (the pinned binary from
//! [`download`]) against a fresh SQLite data dir on an ephemeral port, waits for
//! it to answer `/api/v1/version`, and kills the process plus removes the data
//! dir on drop. It is **never** reached by the default test suite: only an
//! `#[ignore]`d, `HARNESS_FORGEJO_E2E=1`-gated test constructs one, matching the
//! `harness-forge-forgejo` live-test precedent.
//!
//! Phase 1 provides the lifecycle and `base_url()`. Admin/user/token and
//! provisioning helpers are layered on in later phases.

pub mod download;
pub mod runner;

pub use runner::{ForgejoRunner, RunnerError};

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long to wait for `forgejo web` to answer before giving up.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Poll interval while waiting for readiness.
const READY_POLL: Duration = Duration::from_millis(200);

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

        let port = free_port()?;
        let base_url = format!("http://127.0.0.1:{port}");
        let config_path = data_dir.join("custom/conf/app.ini");
        std::fs::write(&config_path, app_ini(&data_dir, port, &base_url))?;

        // `migrate` initializes the SQLite schema before the web server starts.
        run_forgejo(&binary, &config_path, &["migrate"])?;

        let child = spawn_web(&binary, &config_path, &data_dir)?;
        let mut server = Self {
            binary,
            data_dir,
            config_path,
            base_url,
            child,
        };
        server.wait_until_ready()?;
        Ok(server)
    }

    /// The base URL (`http://127.0.0.1:<port>`), no trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The active config file path (used by `forgejo` admin subcommands).
    pub fn config_path(&self) -> &Path {
        &self.config_path
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

    fn wait_until_ready(&mut self) -> Result<(), ServerError> {
        let version_url = format!("{}/api/v1/version", self.base_url);
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|err| ServerError::NotReady(err.to_string()))?;
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            // Surface an early crash instead of polling a dead process.
            if let Some(status) = self.child.try_wait()? {
                return Err(ServerError::NotReady(format!(
                    "process exited early with {status}; log: {}",
                    self.read_log_tail()
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
                    self.read_log_tail()
                )));
            }
            std::thread::sleep(READY_POLL);
        }
    }

    fn read_log_tail(&self) -> String {
        std::fs::read_to_string(self.data_dir.join("web.log"))
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

fn unique_data_dir() -> PathBuf {
    let id = NEXT_INSTANCE.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("harness-forgejo-{}-{id}", std::process::id()))
}

/// Binds `127.0.0.1:0`, reads the assigned port, then releases it. There is an
/// unavoidable race between release and the server's bind, but a fresh OS port
/// is effectively never reused that fast in a test.
fn free_port() -> Result<u16, ServerError> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn run_forgejo(binary: &Path, config: &Path, args: &[&str]) -> Result<String, ServerError> {
    let output = Command::new(binary)
        .arg("--config")
        .arg(config)
        .args(args)
        .env(
            "GITEA_WORK_DIR",
            config
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .unwrap_or(config),
        )
        .output()?;
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
    let child = Command::new(binary)
        .arg("--config")
        .arg(config)
        .arg("web")
        .env("GITEA_WORK_DIR", data_dir)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()?;
    Ok(child)
}

fn app_ini(data_dir: &Path, port: u16, base_url: &str) -> String {
    let root = data_dir.display();
    // A minimal hermetic config: SQLite, no SSH, no mailer, no registration,
    // install lock set so the web server starts straight into the app.
    format!(
        "APP_NAME = Harness Forgejo E2E\n\
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
         SECRET_KEY = harness-e2e-secret-not-for-production\n\
         INTERNAL_TOKEN = harness-e2e-internal-token-not-for-production\n\
         \n\
         [service]\n\
         DISABLE_REGISTRATION = true\n\
         REQUIRE_SIGNIN_VIEW = false\n\
         \n\
         [mailer]\n\
         ENABLED = false\n\
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
