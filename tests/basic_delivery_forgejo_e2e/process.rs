use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use temper_testing::forgejo_runtime::RunWorkspace;
use temper_testing::forgejo_server::ForgejoServer;

use super::{ADMIN_PASSWORD, ADMIN_USER, EXAMPLE_CI, INIT_PROVIDER_KEY, NAME, OWNER};

const STANDALONE_READY_TIMEOUT: Duration = Duration::from_secs(90);

pub(super) struct RunWorkspaceGuard(pub(super) RunWorkspace);

impl RunWorkspaceGuard {
    pub(super) fn new(prefix: &str) -> Self {
        Self(RunWorkspace::new(prefix))
    }
}

pub(super) fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("ephemeral port binds")
        .local_addr()
        .expect("bound listener has local addr")
        .port()
}

pub(super) fn create_site_admin(server: &ForgejoServer) -> String {
    if let Err(error) = server.run_cli(&[
        "admin",
        "user",
        "create",
        "--username",
        ADMIN_USER,
        "--password",
        ADMIN_PASSWORD,
        "--email",
        "basicadmin@example.invalid",
        "--admin",
        "--must-change-password=false",
    ]) && !error.to_string().to_lowercase().contains("exist")
    {
        panic!("creating the site admin failed: {error}");
    }
    let token = server
        .run_cli(&[
            "admin",
            "user",
            "generate-access-token",
            "--username",
            ADMIN_USER,
            "--scopes",
            "all",
            "--raw",
        ])
        .expect("admin token mints");
    let token = token.trim().to_string();
    assert!(!token.is_empty(), "admin token must be non-empty");
    token
}

pub(super) fn run_temper_init(
    server: &ForgejoServer,
    bundle_dir: &Path,
    workspaces_dir: &Path,
    bind_port: u16,
    fake_llm_url: &str,
    log: &Path,
) {
    let fake_home = bundle_dir.join("home-init");
    std::fs::create_dir_all(fake_home.join(".config")).expect("fake init home creates");
    let log_file = log_file_truncate(log);
    let status = Command::new(env!("CARGO_BIN_EXE_temper"))
        .arg("--config")
        .arg(bundle_dir)
        .arg("init")
        .arg("--non-interactive")
        .arg("--force")
        .arg("--apply")
        .arg("--yes")
        .arg("--forge")
        .arg(server.base_url())
        .arg("--repo")
        .arg(format!("{OWNER}/{NAME}"))
        .arg("--workflow")
        .arg("basic-delivery")
        .arg("--bind")
        .arg(format!("127.0.0.1:{bind_port}"))
        .arg("--workspace")
        .arg(workspaces_dir)
        .arg("--admin-user")
        .arg(ADMIN_USER)
        .arg("--provider")
        .arg("deepseek")
        .arg("--provider-url")
        .arg(fake_llm_url)
        .env("TEMPER_INIT_ADMIN_PASSWORD", ADMIN_PASSWORD)
        .env("TEMPER_INIT_PROVIDER_KEY", INIT_PROVIDER_KEY)
        .env("HOME", &fake_home)
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .env("XDG_STATE_HOME", fake_home.join(".local/state"))
        .stdout(Stdio::from(log_file.try_clone().expect("init log clones")))
        .stderr(Stdio::from(log_file))
        .status()
        .expect("temper init process spawns");
    assert!(
        status.success(),
        "temper init --apply failed with {status:?}\n--- init log ---\n{}",
        read_tail(log, 160)
    );
}

pub(super) fn tune_init_config(config_path: &Path, poll_cadence_secs: u64, mech_secs: u64) {
    let text = std::fs::read_to_string(config_path).expect("config.toml reads");
    let mut doc: toml::Value = text.parse().expect("config.toml parses as TOML");
    let engine = doc
        .get_mut("engine")
        .and_then(toml::Value::as_table_mut)
        .expect("config.toml has [engine]");
    engine.insert(
        "poll_cadence_secs".to_string(),
        toml::Value::Integer(poll_cadence_secs as i64),
    );
    engine.insert(
        "mechanical_cadence_secs".to_string(),
        toml::Value::Integer(mech_secs as i64),
    );
    std::fs::write(
        config_path,
        toml::to_string_pretty(&doc).expect("tuned config serializes"),
    )
    .expect("tuned config writes");
}

pub(super) fn populate_repo(base_url: &str, admin_token: &str, workspace: &Path, log: &Path) {
    let seed_dir = workspace.join("repo-seed");
    let checkout = seed_dir.join(NAME);
    let _ = std::fs::remove_dir_all(&seed_dir);
    std::fs::create_dir_all(&checkout).expect("seed checkout creates");
    let _ = std::fs::remove_file(log);

    if run_git_maybe(&checkout, &["init", "-b", "main"], log, "git init -b main").is_err() {
        run_git(&checkout, &["init"], log, "git init");
        run_git(
            &checkout,
            &["checkout", "-B", "main"],
            log,
            "git checkout -B main",
        );
    }
    run_git(
        &checkout,
        &["config", "user.email", "basicadmin@example.invalid"],
        log,
        "git config user.email",
    );
    run_git(
        &checkout,
        &["config", "user.name", "Basic Delivery Admin"],
        log,
        "git config user.name",
    );
    run_git(
        &checkout,
        &[
            "remote",
            "add",
            "origin",
            &format!("{base_url}/{OWNER}/{NAME}.git"),
        ],
        log,
        "git remote add origin",
    );

    std::fs::create_dir_all(checkout.join(".forgejo/workflows"))
        .expect("workflow directory creates");
    std::fs::write(checkout.join(".forgejo/workflows/ci.yml"), EXAMPLE_CI)
        .expect("example CI writes");
    std::fs::write(
        checkout.join("README.md"),
        format!(
            "# {OWNER}/{NAME}\n\nMinimal project baseline for the Temper basic-delivery demo.\n"
        ),
    )
    .expect("README writes");

    run_git(
        &checkout,
        &["add", "README.md", ".forgejo/workflows/ci.yml"],
        log,
        "git add",
    );
    run_git(
        &checkout,
        &[
            "commit",
            "--quiet",
            "-m",
            "chore: initialize basic-delivery demo repository",
        ],
        log,
        "git commit baseline",
    );
    run_git_with_token(
        &checkout,
        admin_token,
        &["push", "--quiet", "--set-upstream", "origin", "HEAD:main"],
        log,
        "git push baseline",
    );
}

pub(super) fn spawn_temper_standalone(bundle_dir: &Path, log: &Path) -> ChildGuard {
    let fake_home = bundle_dir.join("home-standalone");
    std::fs::create_dir_all(fake_home.join(".config")).expect("fake standalone home creates");
    let log_file = log_file_truncate(log);
    let child = Command::new(env!("CARGO_BIN_EXE_temper"))
        .arg("--config")
        .arg(bundle_dir)
        .arg("serve")
        .arg("standalone")
        .env("HOME", &fake_home)
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .env("XDG_STATE_HOME", fake_home.join(".local/state"))
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .stdout(Stdio::from(
            log_file.try_clone().expect("standalone log clones"),
        ))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("temper serve standalone spawns");
    ChildGuard {
        label: "temper serve standalone",
        child,
        log: log.to_path_buf(),
    }
}

pub(super) fn wait_for_standalone(child: &mut ChildGuard) {
    for needle in [
        "webhook listener up",
        "worker:  capacity:",
        "ready -- watching",
    ] {
        let log = child.log.clone();
        wait_for_log_line(&log, needle, child);
    }
}

fn wait_for_log_line(log: &Path, needle: &str, child: &mut ChildGuard) {
    let deadline = Instant::now() + STANDALONE_READY_TIMEOUT;
    loop {
        let contents = std::fs::read_to_string(log).unwrap_or_default();
        if contents.contains(needle) {
            return;
        }
        if let Some(status) = child.try_wait() {
            panic!(
                "{} exited before readiness line {needle:?} with {status:?}\n--- log ---\n{}",
                child.label,
                child.log_tail()
            );
        }
        assert!(
            Instant::now() < deadline,
            "{} did not emit readiness line {needle:?} within {STANDALONE_READY_TIMEOUT:?}\n--- log ---\n{}",
            child.label,
            child.log_tail()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(super) struct ChildGuard {
    pub(super) label: &'static str,
    pub(super) child: Child,
    log: PathBuf,
}

impl ChildGuard {
    pub(super) fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().expect("child try_wait")
    }

    pub(super) fn log_tail(&self) -> String {
        format!(
            "(full log: {})\n{}",
            self.log.display(),
            read_tail(&self.log, 160)
        )
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(super) fn read_tail(path: &Path, lines: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let tail: Vec<&str> = contents.lines().rev().take(lines).collect();
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        }
        Err(error) => format!("<could not read {}: {error}>", path.display()),
    }
}

fn run_git(checkout: &Path, args: &[&str], log: &Path, label: &str) {
    run_git_maybe(checkout, args, log, label).unwrap_or_else(|status| {
        panic!(
            "{label} failed with {status}\n--- git log ---\n{}",
            read_tail(log, 120)
        )
    });
}

fn run_git_maybe(checkout: &Path, args: &[&str], log: &Path, label: &str) -> Result<(), String> {
    run_logged(
        Command::new("git").arg("-C").arg(checkout).args(args),
        log,
        label,
    )
}

fn run_git_with_token(checkout: &Path, token: &str, args: &[&str], log: &Path, label: &str) {
    run_logged(
        Command::new("git")
            .arg("-c")
            .arg(format!("http.extraheader=AUTHORIZATION: token {token}"))
            .arg("-C")
            .arg(checkout)
            .args(args),
        log,
        label,
    )
    .unwrap_or_else(|status| {
        panic!(
            "{label} failed with {status}\n--- git log ---\n{}",
            read_tail(log, 120)
        )
    });
}

fn run_logged(command: &mut Command, log: &Path, label: &str) -> Result<(), String> {
    append_log(log, &format!("$ {label}\n"));
    let output = command.output().map_err(|error| error.to_string())?;
    append_log(log, &String::from_utf8_lossy(&output.stdout));
    append_log(log, &String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(())
    } else {
        Err(output.status.to_string())
    }
}

fn append_log(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("log dir creates");
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("log appends");
    file.write_all(text.as_bytes()).expect("log writes");
}

fn log_file_truncate(path: &Path) -> std::fs::File {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("log dir creates");
    }
    std::fs::File::create(path).expect("log file creates")
}
