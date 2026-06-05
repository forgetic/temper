use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use temper_forge::{CiJobQuery, Forge, PullRequestQuery, RepositoryId};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_runner::{RunnerConfig, Scenario};
use temper_testing::agents::fake_registry;
use temper_testing::forgejo_server::{ForgejoServer, Provisioned};
use temper_testing::worker_bin::{FORGEJO_PASSWORD_ENV, FORGEJO_TOKEN_ENV, FORGEJO_USERNAME_ENV};
use temper_testing::workflow;
use temper_workflow::RoleId;

#[path = "worker_binary.rs"]
mod worker_binary;

pub const LONG_POLL_MS: u64 = 120_000;
const CI_STATUS_POLL_MS: u64 = 1_000;
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
        temper_production::forgejo_rest::ensure_repo_webhook(
            &temper_production::forgejo_rest::http_client().expect("HTTP client builds"),
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

pub fn poll_until_converged(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    repo: &RepoTarget,
    scenario: &Scenario,
) -> Result<(), String> {
    let deadline = Instant::now() + CONVERGENCE_TIMEOUT;
    loop {
        match assert_converged(server, provisioned, repo, scenario) {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() >= deadline => {
                return Err(format!("{} => {error}", repo.display()));
            }
            Err(_) => std::thread::sleep(ASSERT_POLL),
        }
    }
}

fn assert_converged(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    repo: &RepoTarget,
    scenario: &Scenario,
) -> Result<(), String> {
    let forge = admin_forge(server, provisioned, repo);
    futures_block_on((scenario.assert)(&forge, &repo.id)).map_err(|error| error.to_string())
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
    pub fn spawn_with_behavior(
        server: &ForgejoServer,
        provisioned: &Provisioned,
        repos: &[RepoTarget],
        stop_file: &Path,
        wake_dir: &Path,
        wake_secret: &Path,
        log_dir: &Path,
        worker_root_dir: &Path,
        config: &RunnerConfig,
        architect: &str,
        reviewer: &str,
    ) -> Self {
        let base = server.base_url().to_string();
        std::fs::create_dir_all(worker_root_dir).expect("worker root dir creates");
        let mut workers = Vec::new();
        for role in role_workers(config) {
            let identity = provisioned
                .role(&RoleId::new(&role))
                .unwrap_or_else(|| panic!("role '{role}' is provisioned"));
            let log = log_dir.join(format!("{role}.log"));
            let root = worker_root_dir.join(format!("role-{role}"));
            std::fs::create_dir_all(&root).expect("worker root creates");
            let child = spawn_worker(
                &base,
                repos,
                &root,
                stop_file,
                wake_secret,
                &wake_dir.join(format!("{role}.sock")),
                LONG_POLL_MS,
                &[
                    ("--kind", "role"),
                    ("--role", &role),
                    ("--user", &identity.user),
                    ("--architect", architect),
                    ("--reviewer", reviewer),
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
        let root = worker_root_dir.join("mechanical");
        std::fs::create_dir_all(&root).expect("worker root creates");
        let ci_reader = provisioned
            .role(&RoleId::new("engineer"))
            .expect("engineer identity is provisioned for mechanical CI reads");
        let mechanical_env: Vec<(&str, &str)> = vec![
            (FORGEJO_TOKEN_ENV, provisioned.admin_token.as_str()),
            (FORGEJO_USERNAME_ENV, ci_reader.user.as_str()),
            (FORGEJO_PASSWORD_ENV, ci_reader.password.as_str()),
        ];
        let child = spawn_worker(
            &base,
            repos,
            &root,
            stop_file,
            wake_secret,
            &wake_dir.join("mechanical.sock"),
            CI_STATUS_POLL_MS,
            &[("--kind", "mechanical"), ("--idle-poll-max-ms", "8000")],
            &mechanical_env,
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

    pub fn log_offsets(&self) -> Vec<LogOffset> {
        self.workers
            .iter()
            .map(|worker| LogOffset {
                label: worker.label.clone(),
                log: worker.log.clone(),
                bytes: std::fs::metadata(&worker.log)
                    .map(|meta| meta.len())
                    .unwrap_or(0),
            })
            .collect()
    }

    pub fn wake_scan_lines_since(&self, offsets: &[LogOffset]) -> Vec<String> {
        offsets
            .iter()
            .flat_map(|offset| {
                let log = std::fs::read_to_string(&offset.log).unwrap_or_default();
                let start = usize::try_from(offset.bytes).unwrap_or(0).min(log.len());
                log[start..]
                    .lines()
                    .filter(|line| line.contains("completed tick trigger=wake"))
                    .map(|line| format!("{}: {line}", offset.label))
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

pub struct LogOffset {
    label: String,
    log: PathBuf,
    bytes: u64,
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
    root: &Path,
    stop_file: &Path,
    wake_secret: &Path,
    wake_socket: &Path,
    poll_ms: u64,
    extra: &[(&str, &str)],
    env: &[(&str, &str)],
    log_path: &Path,
) -> Child {
    let log = std::fs::File::create(log_path).expect("worker log opens");
    let mut command = Command::new(worker_binary::temper_testing_worker());
    command
        .arg("--backend")
        .arg("forgejo")
        .arg("--base-url")
        .arg(base_url)
        .arg("--root")
        .arg(root)
        .arg("--clock")
        .arg("wall")
        .arg("--poll-ms")
        .arg(poll_ms.to_string())
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
