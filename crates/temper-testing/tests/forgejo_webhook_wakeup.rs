//! Long-poll Forgejo webhook wakeup regression.
//!
//! This is the Phase 4 real-Forgejo wake path check for
//! `plans/hint-driven-wakeups/`: a throwaway Forgejo and real host-mode
//! `forgejo-runner` send webhooks to the production trigger, which delivers
//! authenticated Unix-datagram wakes to fake-agent Forgejo workers. The worker
//! poll interval is intentionally huge; convergence must finish before polling
//! can explain progress.

#![cfg(unix)]

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use temper_forge::{CiJobQuery, Forge, PullRequestQuery};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_production::trigger_args::TriggerArgs;
use temper_runner::{RunnerConfig, Scenario};
use temper_testing::agents::fake_registry;
use temper_testing::forgejo_server::{provision, ForgejoRunner, ForgejoServer, Provisioned};
use temper_testing::scenarios::happy_path;
use temper_testing::worker_bin::{FORGEJO_PASSWORD_ENV, FORGEJO_TOKEN_ENV, FORGEJO_USERNAME_ENV};
use temper_testing::{runner_config, workflow};
use temper_workflow::RoleId;

const LONG_POLL_MS: u64 = 120_000;
const WAKE_CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(90);
const ASSERT_POLL: Duration = Duration::from_secs(1);
const WORKER_RUN_SECS: u64 = 180;

#[test]
#[ignore = "boots real Forgejo + forgejo-runner and opens local sockets; run with --ignored"]
fn happy_path_progresses_by_webhook_wake_before_long_poll() {
    let server = ForgejoServer::start().expect("forgejo server boots");
    let mut runner = ForgejoRunner::register(&server).expect("forgejo runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");
    let provisioned = block_on_provision(&server);

    let run_dir = server.data_dir().join("webhook-wakeup");
    let log_dir = run_dir.join("logs");
    let wake_dir = run_dir.join("wake");
    std::fs::create_dir_all(&log_dir).expect("log dir is created");
    std::fs::create_dir_all(&wake_dir).expect("wake dir is created");
    let stop_file = run_dir.join("stop");
    let webhook_secret = run_dir.join("webhook-secret");
    let wake_secret = run_dir.join("wake-secret");
    std::fs::write(&webhook_secret, "webhook-secret\n").expect("webhook secret is written");
    std::fs::write(&wake_secret, "wake-secret\n").expect("wake secret is written");

    let trigger_addr = free_addr();
    start_trigger(
        trigger_addr,
        webhook_secret.clone(),
        wake_secret.clone(),
        wake_dir.clone(),
    );
    wait_for_trigger(trigger_addr);
    register_webhook(&server, &provisioned, trigger_addr, &webhook_secret);

    let scenario = happy_path();
    let config = runner_config();
    let mut workers = WorkerFleet::spawn(
        &server,
        &provisioned,
        &stop_file,
        &wake_dir,
        &wake_secret,
        &log_dir,
        &config,
    );
    workers.wait_for_initial_ticks(Duration::from_secs(20));

    let started = Instant::now();
    seed(&server, &provisioned, &scenario);
    let converged =
        poll_until_converged(&server, &provisioned, &scenario, WAKE_CONVERGENCE_TIMEOUT);
    let elapsed = started.elapsed();

    touch(&stop_file);
    let exits = workers.wait_all();

    if let Err(error) = converged {
        panic!(
            "webhook wake happy path did not converge within {WAKE_CONVERGENCE_TIMEOUT:?}: {error}\n\
             elapsed={elapsed:?}, poll_interval={LONG_POLL_MS}ms\n\
             trigger address=http://{trigger_addr}/forgejo/webhook (trigger logs were emitted to test stderr)\n\
             worker logs:\n{}\n--- runner running={} log ---\n{}\n--- CI diagnostics ---\n{}",
            workers.logs(),
            runner.is_running(),
            runner.log_tail(),
            ci_diagnostics(&server, &provisioned),
        );
    }

    assert!(
        elapsed < Duration::from_millis(LONG_POLL_MS),
        "converged in {elapsed:?}, which is not before the {LONG_POLL_MS}ms poll backstop"
    );
    assert!(
        workers.logs().contains("consumed authenticated wake"),
        "no worker log recorded an authenticated wake; logs:\n{}",
        workers.logs()
    );

    for (label, status) in &exits {
        assert!(status.success(), "worker '{label}' exited with {status:?}");
    }

    let _ = std::fs::remove_file(&stop_file);
    drop(workers);
    drop(runner);
    drop(server);
}

fn block_on_provision(server: &ForgejoServer) -> Provisioned {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds")
        .block_on(provision(server))
        .expect("provisioning succeeds")
}

fn admin_forge(server: &ForgejoServer, provisioned: &Provisioned) -> ForgejoForge {
    let config = ForgejoConfig::new(server.base_url(), &provisioned.admin_token)
        .with_default_repo(&provisioned.owner, &provisioned.name);
    let config = if let Some(role) = provisioned.roles.values().next() {
        config.with_web_ui_credentials(&role.user, &role.password)
    } else {
        config
    };
    ForgejoForge::new(config)
}

fn register_webhook(
    server: &ForgejoServer,
    provisioned: &Provisioned,
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
            &provisioned.admin_token,
            &provisioned.owner,
            &provisioned.name,
            &url,
            &secret,
        )
        .await
    })
    .unwrap_or_else(|error| panic!("repo webhook registration failed for {url}: {error}"));
}

fn start_trigger(
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
        if let Err(error) = temper_production::trigger::run(&args) {
            eprintln!("webhook wakeup test trigger exited: {error}");
        }
    });
}

fn wait_for_trigger(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "trigger did not start listening at {addr}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn free_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free TCP port binds");
    listener.local_addr().expect("local addr is available")
}

fn seed(server: &ForgejoServer, provisioned: &Provisioned, scenario: &Scenario) {
    let forge = admin_forge(server, provisioned);
    futures_block_on((scenario.seed)(&forge, &provisioned.repository))
        .expect("seeding the scenario succeeds");
}

fn poll_until_converged(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    scenario: &Scenario,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let forge = admin_forge(server, provisioned);
        match futures_block_on((scenario.assert)(&forge, &provisioned.repository)) {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error.to_string()),
            Err(_) => std::thread::sleep(ASSERT_POLL),
        }
    }
}

fn futures_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds")
        .block_on(future)
}

fn ci_diagnostics(server: &ForgejoServer, provisioned: &Provisioned) -> String {
    let forge = admin_forge(server, provisioned);
    futures_block_on(async {
        let mut out = String::new();
        match forge
            .list_pull_requests(&provisioned.repository, PullRequestQuery::default())
            .await
        {
            Ok(prs) => {
                for pr in &prs {
                    out.push_str(&format!(
                        "PR #{} head={} labels={:?} state={:?} merge={}\n",
                        pr.number,
                        pr.source.branch,
                        pr.labels,
                        pr.state,
                        pr.merge.is_some()
                    ));
                    match forge
                        .list_ci_jobs(
                            &provisioned.repository,
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
                                    "  job {} status={:?} conclusion={:?} created={}\n",
                                    job.name, job.status, job.conclusion, job.created_at
                                ));
                            }
                        }
                        Err(error) => out.push_str(&format!("  list_ci_jobs error: {error}\n")),
                    }
                }
            }
            Err(error) => out.push_str(&format!("list_pull_requests error: {error}\n")),
        }
        out
    })
}

struct SpawnedWorker {
    label: String,
    log: PathBuf,
    child: Child,
}

struct WorkerFleet {
    workers: Vec<SpawnedWorker>,
}

impl WorkerFleet {
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        server: &ForgejoServer,
        provisioned: &Provisioned,
        stop_file: &Path,
        wake_dir: &Path,
        wake_secret: &Path,
        log_dir: &Path,
        config: &RunnerConfig,
    ) -> Self {
        let base = server.base_url().to_string();
        let repo = format!("{}/{}", provisioned.owner, provisioned.name);
        let mut workers = Vec::new();
        for role in role_workers(config) {
            let identity = provisioned
                .role(&RoleId::new(&role))
                .unwrap_or_else(|| panic!("role '{role}' is provisioned"));
            let log = log_dir.join(format!("{role}.log"));
            let child = spawn_worker(
                &base,
                &repo,
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
            &repo,
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

    fn wait_for_initial_ticks(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        for worker in &self.workers {
            loop {
                let log = std::fs::read_to_string(&worker.log).unwrap_or_default();
                if log.contains("completed tick") {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "worker '{}' did not complete its initial no-work tick; logs:\n{}",
                    worker.label,
                    self.logs()
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    fn wait_all(&mut self) -> Vec<(String, std::process::ExitStatus)> {
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

    fn logs(&self) -> String {
        self.workers
            .iter()
            .map(|worker| format!("--- {} ---\n{}", worker.label, tail(&worker.log, 80)))
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
    let workflow = workflow();
    let compiled = workflow.compile();
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
    repo: &str,
    stop_file: &Path,
    wake_secret: &Path,
    wake_socket: &Path,
    extra: &[(&str, &str)],
    env: &[(&str, &str)],
    log_path: &Path,
) -> Child {
    let log = std::fs::File::create(log_path).expect("worker log opens");
    let mut command = Command::new(env!("CARGO_BIN_EXE_temper-testing-worker"));
    command
        .arg("--backend")
        .arg("forgejo")
        .arg("--base-url")
        .arg(base_url)
        .arg("--repo")
        .arg(repo)
        .arg("--root")
        .arg(std::env::temp_dir().join("temper-forgejo-webhook-unused"))
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

fn touch(path: &Path) {
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

#[test]
#[cfg(not(unix))]
#[ignore]
fn happy_path_progresses_by_webhook_wake_before_long_poll() {
    eprintln!("skipping Forgejo webhook wakeup e2e: Unix datagram wake sockets are required");
}
