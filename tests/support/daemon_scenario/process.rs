use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use temper_forge::{ForgeAdmin, WebhookEvents, WebhookSpec};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_testing::forgejo_runtime::RunWorkspace;
use temper_testing::forgejo_server::{ForgejoServer, Provisioned, RoleIdentity};

use super::runtime::block_on_with_cx;
use super::{DAEMON_POLL_CADENCE_SECS, ENGINEER};

/// Narrow mechanical backstop cadence. Forgejo 7.0.x does not emit
/// Actions-completion webhooks through repository hooks, so mechanical landing
/// keeps a short test-only poll for CI status transitions only (legacy
/// `CI_STATUS_POLL`). The convergence assertions poll every second, so matching
/// that cadence avoids adding a second of avoidable e2e latency.
const DAEMON_MECHANICAL_CADENCE_SECS: u64 = 1;
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) const WEBHOOK_SECRET: &str = "daemon-e2e-webhook-secret";

pub(super) fn spawn_daemon(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
    port: u16,
    workflow_file: &Path,
    secret_file: &Path,
    log: &Path,
) -> ChildGuard {
    // The new CLI is config-file driven: write the engine's deployment settings
    // to a config file and run `temper --config … --secrets … daemon --service
    // engine`. Secrets (forge admin token, CI web-UI creds, the engineer's
    // per-role identity) go in a companion `credentials.toml` passed via the
    // top-level `--secrets`; the deployment env overrides have been removed
    // from `temper-config`.
    let config_dir = log.parent().expect("daemon log has a parent dir");
    let config_path = config_dir.join("daemon-config.toml");
    let config = format!(
        "schema_version = 1\n\
         [forge]\n\
         type = \"forgejo\"\n\
         url = \"{base_url}\"\n\
         admin = \"admin\"\n\
         ci_user = \"{ENGINEER}\"\n\
         [engine]\n\
         bind = \"127.0.0.1:{port}\"\n\
         repos = [\"{owner}/{name}\"]\n\
         roles = [\"{ENGINEER}\"]\n\
         workflow = \"{workflow}\"\n\
         webhook_secret_file = \"{secret}\"\n\
         poll_cadence_secs = {poll}\n\
         mechanical_cadence_secs = {mech}\n\
         daemon_id = \"temper-daemon-e2e\"\n",
        base_url = server.base_url(),
        owner = provisioned.owner,
        name = provisioned.name,
        workflow = workflow_file.display(),
        secret = secret_file.display(),
        poll = DAEMON_POLL_CADENCE_SECS,
        mech = DAEMON_MECHANICAL_CADENCE_SECS,
    );
    std::fs::write(&config_path, config).expect("write daemon config");

    // Credentials file: the forge admin token (keyed by `[forge] admin`), the
    // web-UI CI-read pair (keyed by `[forge] ci_user`), and the engineer's
    // per-role identity (keyed by the role name). These were previously injected
    // through FORGEJO_ACCESS_TOKEN / FORGEJO_USERNAME / FORGEJO_PASSWORD /
    // TEMPER_FORGEJO_TOKEN_ENGINEER, which `temper-config` no longer reads.
    let credentials_path = config_dir.join("daemon-credentials.toml");
    let credentials = format!(
        "schema_version = 1\n\
         [forge.users.admin]\n\
         token = \"{admin_token}\"\n\
         [forge.users.{ENGINEER}]\n\
         user = \"{eng_user}\"\n\
         password = \"{eng_password}\"\n\
         token = \"{eng_token}\"\n",
        admin_token = provisioned.admin_token,
        eng_user = engineer.user,
        eng_password = engineer.password,
        eng_token = engineer.token,
    );
    std::fs::write(&credentials_path, credentials).expect("write daemon credentials");

    // Hermeticity: point HOME / XDG_* at an isolated, empty dir beside the config
    // so the spawned daemon can never discover a global
    // ~/.config/temper/credentials.toml. The explicit top-level --config already
    // suppresses default discovery (issue #202); isolating HOME makes the test
    // independent of the box it runs on.
    let fake_home = config_path
        .parent()
        .expect("config has a parent dir")
        .join("home");
    std::fs::create_dir_all(fake_home.join(".config")).expect("fake home creates");

    let log_file = log_file(log);
    let child = Command::new(env!("CARGO_BIN_EXE_temper"))
        .arg("--config")
        .arg(&config_path)
        .arg("--secrets")
        .arg(&credentials_path)
        .arg("daemon")
        .arg("--service")
        .arg("engine")
        .env("HOME", &fake_home)
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .env("XDG_STATE_HOME", fake_home.join(".local/state"))
        .env_remove("FORGEJO_DEFAULT_REPO")
        .env_remove("FORGEJO_URL")
        .stdout(Stdio::from(
            log_file.try_clone().expect("daemon log clones"),
        ))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("temper daemon binary spawns");
    ChildGuard {
        label: "temper --config … --secrets … daemon --service engine",
        child,
        log: log.to_path_buf(),
    }
}

/// Waits until the daemon accepts TCP connections on its bind port. Startup
/// includes Forge repository resolution and workflow label upserts, so this can
/// take a few seconds on a fresh world.
pub(super) fn wait_for_daemon(port: u16, daemon: &mut ChildGuard) {
    let deadline = Instant::now() + DAEMON_READY_TIMEOUT;
    loop {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if let Some(status) = daemon.child.try_wait().expect("daemon try_wait") {
            panic!(
                "temper-daemon exited during startup with {status:?}\n--- daemon log ---\n{}",
                daemon.log_tail()
            );
        }
        assert!(
            Instant::now() < deadline,
            "temper-daemon did not bind 127.0.0.1:{port} within {DAEMON_READY_TIMEOUT:?}\n--- daemon log ---\n{}",
            daemon.log_tail()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_worker(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
    daemon_port: u16,
    workspace: &Path,
    stop_file: &Path,
    ci_sentinel: &str,
    log: &Path,
) -> ChildGuard {
    let log_file = log_file(log);
    let child = Command::new(daemon_worker_binary())
        .arg("--daemon-url")
        .arg(format!("http://127.0.0.1:{daemon_port}"))
        .arg("--worker-id")
        .arg("daemon-e2e-worker-1")
        .arg("--capability")
        .arg(format!(
            "{}/{}:{ENGINEER}",
            provisioned.owner, provisioned.name
        ))
        .arg("--git-base-url")
        .arg(server.base_url())
        .arg("--workspace-root")
        .arg(workspace.join("worker/root"))
        .arg("--stop-file")
        .arg(stop_file)
        .arg("--ci-sentinel")
        .arg(ci_sentinel)
        .env("TEMPER_E2E_GIT_USER", &engineer.user)
        .env("TEMPER_E2E_GIT_TOKEN", &engineer.token)
        .stdout(Stdio::from(
            log_file.try_clone().expect("worker log clones"),
        ))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("temper-testing-daemon-worker binary spawns");
    ChildGuard {
        label: "temper-testing-daemon-worker",
        child,
        log: log.to_path_buf(),
    }
}

/// Resolves the daemon test worker binary, which lives in the `temper-testing`
/// package (so no `CARGO_BIN_EXE_...` is exported to this root-package test).
/// Resolution order: explicit env override, then the workspace target dir next
/// to the daemon binary, building it on demand when absent.
fn daemon_worker_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("TEMPER_TESTING_DAEMON_WORKER_BIN") {
        return PathBuf::from(path);
    }
    let daemon = PathBuf::from(env!("CARGO_BIN_EXE_temper"));
    let candidate = daemon
        .parent()
        .expect("daemon binary has a target directory")
        .join("temper-testing-daemon-worker");
    if !candidate.exists() {
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "-j2",
                "-p",
                "temper-testing",
                "--bin",
                "temper-testing-daemon-worker",
            ])
            .status()
            .expect("cargo build for the daemon test worker runs");
        assert!(
            status.success(),
            "building temper-testing-daemon-worker failed"
        );
        assert!(
            candidate.exists(),
            "temper-testing-daemon-worker missing at {} after build; set TEMPER_TESTING_DAEMON_WORKER_BIN",
            candidate.display()
        );
    }
    candidate
}

pub(super) fn register_webhook(server: &ForgejoServer, provisioned: &Provisioned, port: u16) {
    let url = format!("http://127.0.0.1:{port}/forgejo/webhook");
    let base_url = server.base_url().to_string();
    let admin_token = provisioned.admin_token.clone();
    let owner = provisioned.owner.clone();
    let name = provisioned.name.clone();
    let repository = provisioned.repository.clone();
    block_on_with_cx(move |_cx| async move {
        let config = ForgejoConfig::new(&base_url, &admin_token).with_default_repo(&owner, &name);
        let forge = ForgejoForge::new(config);
        forge
            .ensure_webhook(
                &repository,
                WebhookSpec {
                    url,
                    secret: WEBHOOK_SECRET.to_string(),
                    events: WebhookEvents::All,
                },
            )
            .await
    })
    .unwrap_or_else(|error| {
        panic!(
            "repo webhook registration failed for {}/{}: {error}",
            provisioned.owner, provisioned.name
        )
    });
}

/// Allocates a free local port via bind-then-drop.
pub(super) fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("ephemeral port binds")
        .local_addr()
        .expect("bound listener has an address")
        .port()
}

fn log_file(path: &Path) -> std::fs::File {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("log dir creates");
    }
    std::fs::File::create(path).expect("log file creates")
}

/// Kills the spawned child on drop so a panic cannot leak daemon or worker
/// processes (ported from the legacy fleet support's drop discipline).
pub(super) struct ChildGuard {
    label: &'static str,
    child: Child,
    log: PathBuf,
}

impl ChildGuard {
    pub(super) fn log_tail(&self) -> String {
        match std::fs::read_to_string(&self.log) {
            Ok(contents) => {
                let lines: Vec<&str> = contents.lines().rev().take(120).collect();
                lines.into_iter().rev().collect::<Vec<_>>().join("\n")
            }
            Err(error) => format!("<could not read {}: {error}>", self.log.display()),
        }
    }

    /// Waits for a graceful exit, escalating to kill at `timeout`.
    pub(super) fn wait(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("child try_wait") {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                return self.child.wait().expect("child waits after kill");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.label;
    }
}

/// Owns the run workspace; kept so its drop cleanup runs after the children's.
pub(super) struct RunWorkspaceGuard(pub(super) RunWorkspace);

impl RunWorkspaceGuard {
    pub(super) fn new(prefix: &str) -> Self {
        Self(RunWorkspace::new(prefix))
    }
}
