use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use toml::Value as TomlValue;

use crate::forgejo_server::ForgejoServer;

use super::{
    DEFAULT_ADMIN_EMAIL, INIT_PROVIDER_KEY, ObservabilityFixture, RecoveryFixture, RepoFixture,
    ScenarioBundle, TemperCommand,
};

const STANDALONE_READY_TIMEOUT: Duration = Duration::from_secs(90);

pub(super) fn mint_site_admin_token(
    server: &ForgejoServer,
    admin_user: &str,
) -> Result<String, String> {
    let token = server
        .run_cli(&[
            "admin",
            "user",
            "generate-access-token",
            "--username",
            admin_user,
            "--scopes",
            "all",
            "--raw",
        ])
        .map_err(|error| format!("admin token mints: {error}"))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("admin token must be non-empty".to_string());
    }
    Ok(token)
}

pub(super) struct TemperInitRequest<'a> {
    pub(super) temper: &'a TemperCommand,
    pub(super) server: &'a ForgejoServer,
    pub(super) scenario: &'a ScenarioBundle,
    pub(super) bundle_dir: &'a Path,
    pub(super) workspaces_dir: &'a Path,
    pub(super) bind_port: u16,
    pub(super) fake_llm_url: &'a str,
    pub(super) log: &'a Path,
    pub(super) admin_user: &'a str,
    pub(super) admin_password: &'a str,
    pub(super) scenario_run_id: &'a str,
}

pub(super) fn run_temper_init(request: TemperInitRequest<'_>) -> Result<(), String> {
    let fake_home = request.bundle_dir.join("home-init");
    fs::create_dir_all(fake_home.join(".config")).map_err(|error| {
        format!(
            "create fake init config home {}: {error}",
            fake_home.display()
        )
    })?;
    fs::create_dir_all(fake_home.join(".local/state")).map_err(|error| {
        format!(
            "create fake init state home {}: {error}",
            fake_home.display()
        )
    })?;
    let log_file = log_file_truncate(request.log)?;
    let workflow_arg = request.scenario.workflow_path.display().to_string();
    let mut command = request.temper.command();
    command
        .arg("--config")
        .arg(request.bundle_dir)
        .arg("init")
        .arg("--non-interactive")
        .arg("--force")
        .arg("--apply")
        .arg("--yes")
        .arg("--forge")
        .arg(request.server.base_url())
        .arg("--repo")
        .arg(&request.scenario.repo.slug)
        .arg("--workflow")
        .arg(&workflow_arg)
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", request.bind_port))
        .arg("--workspace")
        .arg(request.workspaces_dir)
        .arg("--admin-user")
        .arg(request.admin_user)
        .arg("--provider")
        .arg(&request.scenario.agent_provider)
        .arg("--provider-url")
        .arg(request.fake_llm_url)
        .env("TEMPER_INIT_ADMIN_PASSWORD", request.admin_password);
    if request.scenario.agent_provider == "deepseek" {
        command.env("TEMPER_INIT_PROVIDER_KEY", INIT_PROVIDER_KEY);
    }
    let status = command
        .env("TEMPER_SCENARIO_RUN_ID", request.scenario_run_id)
        .env(
            "TEMPER_LOG_FORMAT",
            &request.scenario.observability.log_format,
        )
        .env("RUST_LOG", &request.scenario.observability.rust_log)
        .env("HOME", &fake_home)
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .env("XDG_STATE_HOME", fake_home.join(".local/state"))
        .stdout(Stdio::from(log_file.try_clone().map_err(|error| {
            format!("clone init log {}: {error}", request.log.display())
        })?))
        .stderr(Stdio::from(log_file))
        .status()
        .map_err(|error| format!("temper init process spawns: {error}"))?;
    if status.success() {
        if request.scenario.agent_provider == "anthropic" {
            configure_anthropic_oauth_fixture(
                request.bundle_dir,
                &format!("jig-{}", request.scenario_run_id),
            )?;
        }
        Ok(())
    } else {
        Err(format!(
            "temper init --apply failed with {status:?}\n--- init log ---\n{}",
            read_tail(request.log, 160)
        ))
    }
}

fn configure_anthropic_oauth_fixture(bundle: &Path, access_token: &str) -> Result<(), String> {
    let config_path = bundle.join("config.toml");
    let mut config: TomlValue = fs::read_to_string(&config_path)
        .map_err(|error| format!("read {}: {error}", config_path.display()))?
        .parse()
        .map_err(|error| format!("parse {}: {error}", config_path.display()))?;
    let profiles = config
        .get_mut("agent")
        .and_then(TomlValue::as_table_mut)
        .and_then(|agent| agent.get_mut("profiles"))
        .and_then(TomlValue::as_table_mut)
        .ok_or_else(|| "generated config has no [agent.profiles] table".to_string())?;
    if profiles.is_empty() {
        return Err("generated config has no agent profile for the live provider".to_string());
    }
    for (_, profile) in profiles.iter_mut() {
        profile
            .as_table_mut()
            .ok_or_else(|| "generated agent profile is not a table".to_string())?
            .insert(
                "credential".to_string(),
                TomlValue::String("agent-provider".to_string()),
            );
    }
    fs::write(
        &config_path,
        toml::to_string_pretty(&config)
            .map_err(|error| format!("serialize {}: {error}", config_path.display()))?,
    )
    .map_err(|error| format!("write {}: {error}", config_path.display()))?;

    let credentials_path = bundle.join("credentials.toml");
    let mut credentials: TomlValue = fs::read_to_string(&credentials_path)
        .map_err(|error| format!("read {}: {error}", credentials_path.display()))?
        .parse()
        .map_err(|error| format!("parse {}: {error}", credentials_path.display()))?;
    let root = credentials
        .as_table_mut()
        .ok_or_else(|| "generated credentials root is not a table".to_string())?;
    let agent = root
        .entry("agent")
        .or_insert_with(|| TomlValue::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| "generated credentials [agent] is not a table".to_string())?;
    let providers = agent
        .entry("providers")
        .or_insert_with(|| TomlValue::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| "generated credentials [agent.providers] is not a table".to_string())?;
    providers.insert(
        "anthropic".to_string(),
        TomlValue::Table(toml::Table::from_iter([
            ("type".to_string(), TomlValue::String("oauth".to_string())),
            (
                "access".to_string(),
                TomlValue::String(access_token.to_string()),
            ),
            (
                "refresh".to_string(),
                TomlValue::String(access_token.to_string()),
            ),
            (
                "expires".to_string(),
                TomlValue::Integer(9_999_999_999_000_i64),
            ),
        ])),
    );
    let secrets = root
        .entry("secrets")
        .or_insert_with(|| TomlValue::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| "generated credentials [secrets] is not a table".to_string())?;
    let credential_json = serde_json::json!({
        "type": "oauth",
        "access_token": access_token,
        "refresh_token": access_token,
        "expires_at_unix_seconds": 9_999_999_999_i64,
    })
    .to_string();
    secrets.insert(
        "agent-provider".to_string(),
        TomlValue::Table(toml::Table::from_iter([
            (
                "kind".to_string(),
                TomlValue::String("provider-credentials".to_string()),
            ),
            (
                "provider".to_string(),
                TomlValue::String("anthropic".to_string()),
            ),
            ("auth".to_string(), TomlValue::String("oauth".to_string())),
            ("value".to_string(), TomlValue::String(credential_json)),
        ])),
    );
    fs::write(
        &credentials_path,
        toml::to_string_pretty(&credentials)
            .map_err(|error| format!("serialize {}: {error}", credentials_path.display()))?,
    )
    .map_err(|error| format!("write {}: {error}", credentials_path.display()))
}

pub(super) fn assert_init_workflow_yaml_matches(
    path: &Path,
    scenario: &ScenarioBundle,
) -> Result<(), String> {
    let workflow = fs::read_to_string(path)
        .map_err(|error| format!("init workflow {} is readable: {error}", path.display()))?;
    let generated_spec = temper_reference_delivery::parse_workflow_spec(path, &workflow)
        .map_err(|error| format!("init workflow parses as YAML: {error}"))?;
    let generated = generated_spec
        .validate()
        .map_err(|errors| format!("init workflow validates: {errors}"))?;
    let expected_spec = temper_reference_delivery::parse_workflow_spec(
        &scenario.workflow_path,
        &scenario.workflow_text,
    )
    .map_err(|error| error.to_string())?;
    let expected = expected_spec
        .validate()
        .map_err(|errors| format!("scenario workflow validates: {errors}"))?;
    if generated != expected {
        return Err("temper init must write the scenario's basic-delivery workflow".to_string());
    }
    if workflow.trim_start().starts_with('{') {
        return Err(format!(
            "temper init should write workflow.yaml as YAML, not JSON bytes: {workflow}"
        ));
    }
    Ok(())
}

pub(super) fn tune_init_config(
    config_path: &Path,
    poll_cadence_secs: u64,
    ci_poll_cadence_secs: u64,
    mech_secs: u64,
    recovery: Option<&RecoveryFixture>,
) -> Result<super::EffectiveConfigurationEvidence, String> {
    let text = fs::read_to_string(config_path)
        .map_err(|error| format!("read {}: {error}", config_path.display()))?;
    let mut doc: TomlValue = text
        .parse()
        .map_err(|error| format!("parse {} as TOML: {error}", config_path.display()))?;
    let engine = doc
        .get_mut("engine")
        .and_then(TomlValue::as_table_mut)
        .ok_or_else(|| "config.toml has no [engine] table".to_string())?;
    engine.insert(
        "poll_cadence_secs".to_string(),
        TomlValue::Integer(poll_cadence_secs as i64),
    );
    engine.insert(
        "ci_poll_cadence_secs".to_string(),
        TomlValue::Integer(ci_poll_cadence_secs as i64),
    );
    engine.insert(
        "mechanical_cadence_secs".to_string(),
        TomlValue::Integer(mech_secs as i64),
    );
    if let Some(recovery) = recovery {
        let worker = doc
            .get_mut("worker")
            .and_then(TomlValue::as_table_mut)
            .ok_or_else(|| "config.toml has no [worker] table".to_string())?;
        for (field, value) in [
            (
                "session_failure_limit",
                u64::from(recovery.session_failure_limit),
            ),
            (
                "fresh_session_limit",
                u64::from(recovery.fresh_session_limit),
            ),
            (
                "provider_deferral_limit",
                u64::from(recovery.provider_deferral_limit),
            ),
            (
                "provider_deferral_delay_secs",
                recovery.provider_deferral_delay_secs,
            ),
            ("model_recovery_slo_secs", recovery.model_recovery_slo_secs),
        ] {
            worker.insert(field.to_string(), TomlValue::Integer(value as i64));
        }
        let agent = doc
            .get_mut("agent")
            .and_then(TomlValue::as_table_mut)
            .ok_or_else(|| "config.toml has no [agent] table".to_string())?;
        let deadlines = agent
            .entry("deadlines")
            .or_insert_with(|| TomlValue::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| "config.toml [agent.deadlines] value is not a table".to_string())?;
        for (field, value) in [
            (
                "model_retry_max_attempts",
                u64::from(recovery.model_retry_max_attempts),
            ),
            (
                "model_retry_base_delay_ms",
                recovery.model_retry_base_delay_ms,
            ),
            (
                "model_retry_max_delay_ms",
                recovery.model_retry_max_delay_ms,
            ),
            (
                "model_retry_jitter_percent",
                u64::from(recovery.model_retry_jitter_percent),
            ),
        ] {
            deadlines.insert(field.to_string(), TomlValue::Integer(value as i64));
        }
    }
    let serialized =
        toml::to_string_pretty(&doc).map_err(|error| format!("serialize tuned config: {error}"))?;
    fs::write(config_path, serialized)
        .map_err(|error| format!("write tuned config {}: {error}", config_path.display()))?;
    read_effective_configuration(config_path)
}

pub(super) fn read_effective_configuration(
    config_path: &Path,
) -> Result<super::EffectiveConfigurationEvidence, String> {
    let text = fs::read_to_string(config_path)
        .map_err(|error| format!("read back tuned config {}: {error}", config_path.display()))?;
    let doc = text
        .parse::<TomlValue>()
        .map_err(|error| format!("parse tuned config {}: {error}", config_path.display()))?;
    let engine = doc
        .get("engine")
        .and_then(TomlValue::as_table)
        .ok_or_else(|| "tuned config.toml has no [engine] table".to_string())?;
    let cadence = |field: &str| {
        engine
            .get(field)
            .and_then(TomlValue::as_integer)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| format!("tuned config engine.{field} is not a whole-second value"))
    };
    Ok(super::EffectiveConfigurationEvidence {
        ci_poll_cadence_secs: cadence("ci_poll_cadence_secs")?,
        poll_cadence_secs: cadence("poll_cadence_secs")?,
        mechanical_cadence_secs: cadence("mechanical_cadence_secs")?,
    })
}

pub(super) fn populate_repo(
    base_url: &str,
    admin_token: &str,
    workspace: &Path,
    repo: &RepoFixture,
    log: &Path,
) -> Result<(), String> {
    let seed_dir = workspace.join("repo-seed");
    let checkout = seed_dir.join(&repo.name);
    let _ = fs::remove_dir_all(&seed_dir);
    fs::create_dir_all(&checkout)
        .map_err(|error| format!("seed checkout creates {}: {error}", checkout.display()))?;
    let _ = fs::remove_file(log);

    if run_git_maybe(
        &checkout,
        &["init", "-b", &repo.default_branch],
        log,
        "git init -b default branch",
    )
    .is_err()
    {
        run_git(&checkout, &["init"], log, "git init")?;
        run_git(
            &checkout,
            &["checkout", "-B", &repo.default_branch],
            log,
            "git checkout -B default branch",
        )?;
    }
    run_git(
        &checkout,
        &["config", "user.email", DEFAULT_ADMIN_EMAIL],
        log,
        "git config user.email",
    )?;
    run_git(
        &checkout,
        &["config", "user.name", "Basic Delivery Admin"],
        log,
        "git config user.name",
    )?;
    run_git(
        &checkout,
        &[
            "remote",
            "add",
            "origin",
            &format!("{base_url}/{}/{}.git", repo.owner, repo.name),
        ],
        log,
        "git remote add origin",
    )?;

    copy_dir_contents(&repo.seed_path, &checkout)?;
    let seeded_ci = checkout.join(&repo.ci_target);
    let seeded_ci_text = fs::read_to_string(&seeded_ci)
        .map_err(|error| format!("read seeded CI {}: {error}", seeded_ci.display()))?;
    if seeded_ci_text != repo.ci_source {
        return Err(format!(
            "seeded CI {} does not match declared CI source {}",
            seeded_ci.display(),
            repo.ci_source_path.display()
        ));
    }

    run_git(&checkout, &["add", "--all"], log, "git add --all")?;
    run_git(
        &checkout,
        &[
            "commit",
            "--quiet",
            "-m",
            "chore: initialize basic-delivery scenario repository",
        ],
        log,
        "git commit baseline",
    )?;
    run_git_with_token(
        &checkout,
        admin_token,
        &[
            "push",
            "--quiet",
            "--set-upstream",
            "origin",
            &format!("HEAD:{}", repo.default_branch),
        ],
        log,
        "git push baseline",
    )
}

fn copy_dir_contents(source: &Path, destination: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source)
        .map_err(|error| format!("read seed dir {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("read seed dir entry: {error}"))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("stat seed entry {}: {error}", source_path.display()))?;
        if file_type.is_dir() {
            fs::create_dir_all(&destination_path).map_err(|error| {
                format!(
                    "create seed destination {}: {error}",
                    destination_path.display()
                )
            })?;
            copy_dir_contents(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("create seed destination {}: {error}", parent.display())
                })?;
            }
            fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "copy seed file {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

pub(super) fn spawn_temper_standalone(
    temper: &TemperCommand,
    bundle_dir: &Path,
    log: &Path,
    observability: &ObservabilityFixture,
    scenario_run_id: &str,
) -> Result<ChildGuard, String> {
    let fake_home = bundle_dir.join("home-standalone");
    fs::create_dir_all(fake_home.join(".config")).map_err(|error| {
        format!(
            "create fake standalone config home {}: {error}",
            fake_home.display()
        )
    })?;
    fs::create_dir_all(fake_home.join(".local/state")).map_err(|error| {
        format!(
            "create fake standalone state home {}: {error}",
            fake_home.display()
        )
    })?;
    let log_file = log_file_truncate(log)?;
    let child = temper
        .command()
        .arg("--config")
        .arg(bundle_dir)
        .arg("serve")
        .arg("standalone")
        .env("TEMPER_LOG_FORMAT", &observability.log_format)
        .env("TEMPER_SCENARIO_RUN_ID", scenario_run_id)
        .env("RUST_LOG", &observability.rust_log)
        .env("HOME", &fake_home)
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .env("XDG_STATE_HOME", fake_home.join(".local/state"))
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .stdout(Stdio::from(log_file.try_clone().map_err(|error| {
            format!("clone standalone log {}: {error}", log.display())
        })?))
        .stderr(Stdio::from(log_file))
        .spawn()
        .map_err(|error| format!("temper serve standalone spawns: {error}"))?;
    Ok(ChildGuard {
        label: "temper serve standalone",
        child,
        log: log.to_path_buf(),
    })
}

pub(super) fn wait_for_standalone(child: &mut ChildGuard) -> Result<(), String> {
    wait_for_standalone_with_timeout(child, STANDALONE_READY_TIMEOUT)
}

pub(super) fn wait_for_standalone_with_timeout(
    child: &mut ChildGuard,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    for needle in [
        "webhook listener up",
        "worker:  capacity:",
        "ready -- watching",
    ] {
        let log = child.log.clone();
        wait_for_log_line(&log, needle, child, deadline, timeout)?;
    }
    Ok(())
}

fn wait_for_log_line(
    log: &Path,
    needle: &str,
    child: &mut ChildGuard,
    deadline: Instant,
    timeout: Duration,
) -> Result<(), String> {
    loop {
        let contents = fs::read_to_string(log).unwrap_or_default();
        if contents.contains(needle) {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!(
                "{} exited before readiness line {needle:?} with {status:?}\n--- log ---\n{}",
                child.label,
                child.log_tail()
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} did not emit readiness line {needle:?} within {timeout:?}\n--- log ---\n{}",
                child.label,
                child.log_tail()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(super) fn free_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("ephemeral port binds: {error}"))?
        .local_addr()
        .map_err(|error| format!("bound listener has local addr: {error}"))
        .map(|addr| addr.port())
}

pub(super) struct ChildGuard {
    pub(super) label: &'static str,
    child: Child,
    log: PathBuf,
}

impl ChildGuard {
    pub(super) fn pid(&self) -> u32 {
        self.child.id()
    }

    pub(super) fn try_wait(&mut self) -> Result<Option<ExitStatus>, String> {
        self.child
            .try_wait()
            .map_err(|error| format!("{} try_wait failed: {error}", self.label))
    }

    pub(super) fn log_tail(&self) -> String {
        format!(
            "(full log: {})\n{}",
            self.log.display(),
            read_tail(&self.log, 160)
        )
    }

    #[cfg(target_os = "linux")]
    pub(super) fn signal(&self, signal: &str) -> Result<(), String> {
        let status = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(self.child.id().to_string())
            .status()
            .map_err(|error| format!("send {signal} to {}: {error}", self.label))?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| format!("send {signal} to {} failed with {status}", self.label))
    }

    #[cfg(target_os = "linux")]
    pub(super) fn wait_for_exit(&mut self, timeout: Duration) -> Result<ExitStatus, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "{} did not exit within {timeout:?}\n--- log ---\n{}",
                    self.label,
                    self.log_tail()
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub(super) fn kill(&mut self) {
        let _ = self.child.kill();
    }

    pub(super) fn kill_and_wait(&mut self) -> Result<(), String> {
        self.child
            .kill()
            .map_err(|error| format!("{} kill failed: {error}", self.label))?;
        self.child
            .wait()
            .map(|_| ())
            .map_err(|error| format!("{} wait after kill failed: {error}", self.label))
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(super) fn read_tail(path: &Path, lines: usize) -> String {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let tail: Vec<&str> = contents.lines().rev().take(lines).collect();
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        }
        Err(error) => format!("<could not read {}: {error}>", path.display()),
    }
}

fn run_git(checkout: &Path, args: &[&str], log: &Path, label: &str) -> Result<(), String> {
    run_git_maybe(checkout, args, log, label).map_err(|status| {
        format!(
            "{label} failed with {status}\n--- git log ---\n{}",
            read_tail(log, 120)
        )
    })
}

fn run_git_maybe(checkout: &Path, args: &[&str], log: &Path, label: &str) -> Result<(), String> {
    run_logged(
        Command::new("git").arg("-C").arg(checkout).args(args),
        log,
        label,
    )
}

fn run_git_with_token(
    checkout: &Path,
    token: &str,
    args: &[&str],
    log: &Path,
    label: &str,
) -> Result<(), String> {
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
    .map_err(|status| {
        format!(
            "{label} failed with {status}\n--- git log ---\n{}",
            read_tail(log, 120)
        )
    })
}

fn run_logged(command: &mut Command, log: &Path, label: &str) -> Result<(), String> {
    append_log(log, &format!("$ {label}\n"))?;
    let output = command.output().map_err(|error| error.to_string())?;
    append_log(log, &String::from_utf8_lossy(&output.stdout))?;
    append_log(log, &String::from_utf8_lossy(&output.stderr))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(output.status.to_string())
    }
}

fn append_log(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create log dir {}: {error}", parent.display()))?;
    }
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("open log {} for append: {error}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("write log {}: {error}", path.display()))
}

fn log_file_truncate(path: &Path) -> Result<fs::File, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create log dir {}: {error}", parent.display()))?;
    }
    fs::File::create(path).map_err(|error| format!("create log {}: {error}", path.display()))
}

pub(super) fn write_snapshot(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, text);
}

pub(super) fn convergence_timeout(default: Duration) -> Duration {
    std::env::var("TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(default)
}

pub(super) fn engine_block_on<F: std::future::Future>(future: F) -> F::Output {
    temper_engine_io::build_runtime()
        .expect("engine runtime builds")
        .block_on(future)
}
