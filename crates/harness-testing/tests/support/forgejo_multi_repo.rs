use harness_forge::{
    CiJobQuery, Forge, PullRequestQuery, RepositoryId, RepositoryPath, UpsertLabel,
};
use harness_forge_forgejo::{ForgejoConfig, ForgejoForge};
use harness_production::trigger_args::TriggerArgs;
use harness_runner::{RunnerConfig, Scenario};
use harness_testing::agents::fake_registry;
use harness_testing::forgejo_server::{provision, ForgejoServer, Provisioned};
use harness_testing::worker_bin::{FORGEJO_PASSWORD_ENV, FORGEJO_TOKEN_ENV, FORGEJO_USERNAME_ENV};
use harness_testing::{runner_config, workflow};
use harness_workflow::RoleId;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const LONG_POLL_MS: u64 = 120_000;
pub const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(180);
const ASSERT_POLL: Duration = Duration::from_secs(1);
const WORKER_RUN_SECS: u64 = 240;

#[derive(Clone)]
pub struct RepoTarget {
    pub owner: String,
    pub name: String,
    pub id: RepositoryId,
}

impl RepoTarget {
    pub fn from_provisioned(provisioned: &Provisioned) -> Self {
        Self {
            owner: provisioned.owner.clone(),
            name: provisioned.name.clone(),
            id: provisioned.repository.clone(),
        }
    }

    pub fn display(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

pub fn enabled() -> bool {
    if std::env::var("HARNESS_FORGEJO_E2E").ok().as_deref() == Some("1") {
        true
    } else {
        eprintln!(
            "skipping Forgejo multi-repo webhook e2e: set HARNESS_FORGEJO_E2E=1 to enable \
             (boots real Forgejo + forgejo-runner and opens local webhook/wake sockets)"
        );
        false
    }
}

pub fn block_on_provision(server: &ForgejoServer) -> Provisioned {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds")
        .block_on(provision(server))
        .expect("provisioning succeeds")
}

pub async fn provision_extra_repo(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    name: &str,
) -> Result<RepositoryId, Box<dyn std::error::Error>> {
    let client = harness_production::forgejo_rest::http_client()?;
    let config = runner_config();
    harness_production::forgejo_rest::ensure_repo(
        &client,
        server.base_url(),
        &provisioned.admin_token,
        &provisioned.owner,
        name,
        &config.repository.default_branch,
    )
    .await?;
    let repo = repo_id(server, &provisioned.admin_token, &provisioned.owner, name).await?;
    let compiled = workflow().compile();
    let forge = ForgejoForge::new(
        ForgejoConfig::new(server.base_url(), &provisioned.admin_token)
            .with_default_repo(&provisioned.owner, name),
    );
    for label in compiled.labels().labels() {
        forge
            .upsert_label(
                &repo,
                UpsertLabel {
                    name: label.id.to_string(),
                    color: Some("#ededed".into()),
                    description: None,
                },
            )
            .await?;
    }
    harness_production::forgejo_rest::commit_file(
        &client,
        server.base_url(),
        &provisioned.admin_token,
        &provisioned.owner,
        name,
        ".forgejo/workflows/ci.yml",
        harness_testing::forgejo_server::provision::CI_WORKFLOW,
        "add CI workflow (runs-on: host)",
        &config.repository.default_branch,
    )
    .await?;
    harness_production::forgejo_rest::enable_actions(
        &client,
        server.base_url(),
        &provisioned.admin_token,
        &provisioned.owner,
        name,
    )
    .await?;
    Ok(repo)
}

async fn repo_id(
    server: &ForgejoServer,
    token: &str,
    owner: &str,
    name: &str,
) -> Result<RepositoryId, Box<dyn std::error::Error>> {
    let forge = ForgejoForge::new(ForgejoConfig::new(server.base_url(), token));
    Ok(forge
        .get_repository_by_path(&RepositoryPath::new(owner, name))
        .await?
        .ok_or_else(|| format!("repo {owner}/{name} not readable"))?
        .id)
}

fn admin_forge(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    repo: &RepoTarget,
) -> ForgejoForge {
    let config = ForgejoConfig::new(server.base_url(), &provisioned.admin_token)
        .with_default_repo(&repo.owner, &repo.name);
    let config = if let Some(role) = provisioned.roles.values().next() {
        config.with_web_ui_credentials(&role.user, &role.password)
    } else {
        config
    };
    ForgejoForge::new(config)
}

pub fn register_webhook(
    server: &ForgejoServer,
    admin_token: &str,
    repo: &RepoTarget,
    addr: SocketAddr,
    secret_file: &Path,
) {
    let secret = std::fs::read_to_string(secret_file)
        .expect("webhook secret is readable")
        .trim()
        .to_string();
    let url = format!("http://{addr}/forgejo/webhook");
    futures_block_on(async {
        harness_production::forgejo_rest::ensure_repo_webhook(
            &harness_production::forgejo_rest::http_client().expect("HTTP client builds"),
            server.base_url(),
            admin_token,
            &repo.owner,
            &repo.name,
            &url,
            &secret,
        )
        .await
    })
    .unwrap_or_else(|error| {
        panic!(
            "webhook registration failed for {} at {url}: {error}",
            repo.display()
        )
    });
}

pub fn start_trigger(
    addr: SocketAddr,
    webhook_secret: PathBuf,
    wake_secret: PathBuf,
    wake_dir: PathBuf,
) {
    std::thread::spawn(move || {
        let args = TriggerArgs {
            bind: addr,
            webhook_secret_file: webhook_secret,
            wake_secret_file: Some(wake_secret),
            wake_dir: Some(wake_dir),
            wake_sockets: Vec::new(),
        };
        if let Err(error) = harness_production::trigger::run(&args) {
            eprintln!("multi-repo webhook test trigger exited: {error}");
        }
    });
}

pub fn wait_for_trigger(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "trigger did not listen at {addr}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn free_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free TCP port binds");
    listener.local_addr().expect("local addr is available")
}

pub fn seed(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    repo: &RepoTarget,
    scenario: &Scenario,
) {
    let forge = admin_forge(server, provisioned, repo);
    futures_block_on((scenario.seed)(&forge, &repo.id))
        .unwrap_or_else(|error| panic!("seeding {} failed: {error}", repo.display()));
}

pub fn poll_until_all_converged(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    repos: &[RepoTarget],
    scenario: &Scenario,
) -> Result<(), String> {
    let deadline = Instant::now() + CONVERGENCE_TIMEOUT;
    let mut last = vec![String::new(); repos.len()];
    loop {
        let mut all_ok = true;
        for (idx, repo) in repos.iter().enumerate() {
            let forge = admin_forge(server, provisioned, repo);
            match futures_block_on((scenario.assert)(&forge, &repo.id)) {
                Ok(()) => last[idx] = "converged".into(),
                Err(error) => {
                    all_ok = false;
                    last[idx] = error.to_string();
                }
            }
        }
        if all_ok {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(repos
                .iter()
                .zip(last.iter())
                .map(|(repo, error)| format!("{} => {error}", repo.display()))
                .collect::<Vec<_>>()
                .join("\n"));
        }
        std::thread::sleep(ASSERT_POLL);
    }
}

pub fn futures_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds")
        .block_on(future)
}

pub fn ci_diagnostics(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    repos: &[RepoTarget],
) -> String {
    futures_block_on(async {
        let mut out = String::new();
        for repo in repos {
            out.push_str(&format!("repo {}\n", repo.display()));
            let forge = admin_forge(server, provisioned, repo);
            match forge
                .list_pull_requests(&repo.id, PullRequestQuery::default())
                .await
            {
                Ok(prs) => {
                    for pr in &prs {
                        out.push_str(&format!(
                            "  PR #{} head={} labels={:?} state={:?} merge={}\n",
                            pr.number,
                            pr.source.branch,
                            pr.labels,
                            pr.state,
                            pr.merge.is_some()
                        ));
                        match forge
                            .list_ci_jobs(
                                &repo.id,
                                CiJobQuery {
                                    pull_request_id: Some(pr.id.clone()),
                                    ..CiJobQuery::default()
                                },
                            )
                            .await
                        {
                            Ok(jobs) => {
                                for job in jobs {
                                    out.push_str(&format!(
                                        "    job {} status={:?} conclusion={:?}\n",
                                        job.name, job.status, job.conclusion
                                    ));
                                }
                            }
                            Err(error) => {
                                out.push_str(&format!("    list_ci_jobs error: {error}\n"));
                            }
                        }
                    }
                }
                Err(error) => out.push_str(&format!("  list_pull_requests error: {error}\n")),
            }
        }
        out
    })
}

pub struct WorkerFleet {
    workers: Vec<SpawnedWorker>,
}

struct SpawnedWorker {
    label: String,
    log: PathBuf,
    child: Child,
}

impl WorkerFleet {
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        server: &ForgejoServer,
        provisioned: &Provisioned,
        repos: &[RepoTarget],
        stop_file: &Path,
        wake_dir: &Path,
        wake_secret: &Path,
        log_dir: &Path,
        config: &RunnerConfig,
    ) -> Self {
        let base = server.base_url().to_string();
        let mut workers = Vec::new();
        for role in role_workers(config) {
            let identity = provisioned
                .role(&RoleId::new(&role))
                .unwrap_or_else(|| panic!("role '{role}' is provisioned"));
            let log = log_dir.join(format!("{role}.log"));
            let child = spawn_worker(
                &base,
                repos,
                stop_file,
                wake_secret,
                &wake_dir.join(format!("{role}.sock")),
                &[
                    ("--kind", "role"),
                    ("--role", &role),
                    ("--user", &identity.user),
                ],
                &[
                    (FORGEJO_TOKEN_ENV, identity.token.as_str()),
                    (FORGEJO_USERNAME_ENV, identity.user.as_str()),
                    (FORGEJO_PASSWORD_ENV, identity.password.as_str()),
                ],
                &log,
            );
            workers.push(SpawnedWorker {
                label: format!("role:{role}"),
                log,
                child,
            });
        }
        let log = log_dir.join("mechanical.log");
        let child = spawn_worker(
            &base,
            repos,
            stop_file,
            wake_secret,
            &wake_dir.join("mechanical.sock"),
            &[("--kind", "mechanical")],
            &[(FORGEJO_TOKEN_ENV, provisioned.admin_token.as_str())],
            &log,
        );
        workers.push(SpawnedWorker {
            label: "mechanical".into(),
            log,
            child,
        });
        Self { workers }
    }

    pub fn wait_for_initial_ticks(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        for worker in &self.workers {
            loop {
                let log = std::fs::read_to_string(&worker.log).unwrap_or_default();
                if log.contains("completed tick") {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "worker '{}' did not complete its initial tick; logs:\n{}",
                    worker.label,
                    self.logs()
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    pub fn wait_all(&mut self) -> Vec<(String, std::process::ExitStatus)> {
        self.workers
            .iter_mut()
            .map(|worker| {
                let status = worker.child.wait().unwrap_or_else(|error| {
                    panic!("waiting on '{}' failed: {error}", worker.label)
                });
                (worker.label.clone(), status)
            })
            .collect()
    }

    pub fn logs(&self) -> String {
        self.workers
            .iter()
            .map(|worker| {
                format!(
                    "--- {} ({}) ---\n{}",
                    worker.label,
                    worker.log.display(),
                    tail(&worker.log, 80)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Drop for WorkerFleet {
    fn drop(&mut self) {
        for worker in &mut self.workers {
            let _ = worker.child.kill();
            let _ = worker.child.wait();
        }
    }
}

fn role_workers(config: &RunnerConfig) -> Vec<String> {
    let compiled = workflow().compile();
    let registry = fake_registry::<dyn Forge>();
    compiled
        .roles()
        .iter()
        .filter(|role| registry.get(&role.id).is_some())
        .filter(|role| config.role_binding(&role.id).is_some())
        .map(|role| role.id.to_string())
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn spawn_worker(
    base_url: &str,
    repos: &[RepoTarget],
    stop_file: &Path,
    wake_secret: &Path,
    wake_socket: &Path,
    extra: &[(&str, &str)],
    env: &[(&str, &str)],
    log_path: &Path,
) -> Child {
    let log = std::fs::File::create(log_path).expect("worker log opens");
    let mut command = Command::new(env!("CARGO_BIN_EXE_harness-testing-worker"));
    command
        .arg("--backend")
        .arg("forgejo")
        .arg("--base-url")
        .arg(base_url)
        .arg("--root")
        .arg(std::env::temp_dir().join("harness-forgejo-multi-repo-unused"))
        .arg("--clock")
        .arg("wall")
        .arg("--poll-ms")
        .arg(LONG_POLL_MS.to_string())
        .arg("--stop-file")
        .arg(stop_file)
        .arg("--run-secs")
        .arg(WORKER_RUN_SECS.to_string())
        .arg("--wake-socket")
        .arg(wake_socket)
        .arg("--wake-secret-file")
        .arg(wake_secret);
    for repo in repos {
        command.arg("--repo").arg(repo.display());
    }
    for (flag, value) in extra {
        command.arg(flag).arg(value);
    }
    command
        .env_remove(FORGEJO_TOKEN_ENV)
        .env_remove(FORGEJO_USERNAME_ENV)
        .env_remove(FORGEJO_PASSWORD_ENV);
    for (key, value) in env {
        command.env(key, value);
    }
    command
        .stdout(Stdio::from(log.try_clone().expect("worker log clones")))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap_or_else(|error| panic!("spawning worker {extra:?} failed: {error}"))
}

pub fn touch(path: &Path) {
    std::fs::write(path, b"stop").expect("writing stop sentinel succeeds");
}

fn tail(path: &Path, lines: usize) -> String {
    std::fs::read_to_string(path)
        .map(|log| {
            log.lines()
                .rev()
                .take(lines)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|error| format!("<could not read {}: {error}>", path.display()))
}
