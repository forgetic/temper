//! Forgejo multi-process scenarios as per-scenario live-world tests.
//!
//! This is the real-backend twin of `tests/multiprocess.rs`: deterministic fake
//! role agents and a mechanical worker coordinate only through a real Forgejo
//! backend while a real host-mode `forgejo-runner` produces CI. Each ignored
//! test owns one live world for exactly the repositories needed by that scenario:
//!
//! 1. declare cached state containing the admin, role identities, and that
//!    scenario's repositories;
//! 2. start a stack-owned [`ForgejoServer`] from a `/tmp` copy of that state and
//!    register one stack-owned [`ForgejoRunner`];
//! 3. register repo webhooks against the scenario's trigger;
//! 4. spawn a worker fleet with unique stop file, wake sockets, and logs;
//! 5. poll the scenario's backend-neutral assert closure to convergence while
//!    the workers advance through webhook wakes, not their poll backstop.
//!
//! Ordinary Rust ownership performs panic cleanup: the active worker fleet is a
//! stack value and kills children on drop, and each scenario's runner/server
//! handles are dropped if that test panics.
//!
//! ```sh
//! cargo test -p temper-testing --test forgejo_multiprocess -- --ignored
//! ```

#![cfg(unix)]

use std::net::SocketAddr;
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_runner::Scenario;
use temper_testing::forgejo_server::{
    start_cached_provisioned_repositories, ForgejoRunner, ForgejoServer, Provisioned,
    ProvisionedRepositories,
};
#[path = "support/forgejo_multiprocess.rs"]
mod multiprocess_support;

use multiprocess_support::{convergence_timeout, Variant, WorkerFleet, WorkerPollProfile};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use temper_testing::forgejo_runtime::{RunWorkspace, TriggerServer};
use temper_testing::runner_config;

/// How often the driver re-runs the assert closure while polling.
const ASSERT_POLL: Duration = Duration::from_secs(1);
/// Deliberately long worker poll backstop; scenario progress should come from
/// Forgejo webhooks delivered through wake sockets before this deadline.
const LONG_POLL_MS: u64 = 120_000;
/// Forgejo 7.0.x does not emit Actions-completion webhooks through repository
/// hooks, so CI-reading roles and mechanical landing keep a narrow poll
/// backstop for CI status transitions only.
const CI_STATUS_POLL_MS: u64 = 1_000;
/// Backstop run length per child, in case the driver dies before stopping it.
const WORKER_RUN_SECS: u64 = 1200;

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn forgejo_multiprocess_happy_path_converges() {
    run_forgejo_multiprocess_variant(Variant::happy_path());
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn forgejo_multiprocess_changes_requested_then_approved_converges() {
    run_forgejo_multiprocess_variant(Variant::changes_requested_then_approved());
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn forgejo_multiprocess_ci_fails_then_passes_converges() {
    run_forgejo_multiprocess_variant(Variant::ci_fails_then_passes());
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn forgejo_multiprocess_dependency_chain_mechanically_unblocked_converges() {
    run_forgejo_multiprocess_variant(Variant::dependency_chain_mechanically_unblocked());
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn forgejo_multiprocess_cross_repo_fanout_converges() {
    run_forgejo_multiprocess_variant(Variant::cross_repo_fanout());
}

fn run_forgejo_multiprocess_variant(variant: Variant) {
    let test_start = Instant::now();
    let repo_names = variant.repo_names();
    let mut world = ScenarioLiveWorld::start(&repo_names);
    eprintln!(
        "forgejo_multiprocess scenario '{}' world timing: {}",
        variant.name,
        world.timing.render()
    );

    let timing = run_variant(&mut world, &variant);
    eprintln!(
        "forgejo_multiprocess scenario '{}' timing: {}",
        variant.name,
        timing.render()
    );

    let world_teardown_start = Instant::now();
    drop(world);
    let world_teardown = world_teardown_start.elapsed();
    eprintln!(
        "forgejo_multiprocess test '{}' timing: scenario={} \
         world_teardown={world_teardown:?} total={:?}",
        variant.name,
        timing.render(),
        test_start.elapsed()
    );
}

/// Drives one scenario through the true multi-process topology against a real
/// Forgejo, asserting it converges to the same end state as the in-process
/// scenario.
fn run_variant(world: &mut ScenarioLiveWorld, variant: &Variant) -> ScenarioTiming {
    let scenario_start = Instant::now();
    let mut timing = ScenarioTiming::default();

    let provision_start = Instant::now();
    let mut repos = vec![world.provision_repo(variant.primary_repo)];
    if let Some(name) = variant.extra_repo {
        repos.push(world.provision_repo(name));
    }
    timing.repo_provision = provision_start.elapsed();
    let primary = repos[0].clone();
    let repo_args = repos
        .iter()
        .map(|repo| format!("{}/{}", repo.owner, repo.name))
        .collect::<Vec<_>>();

    let scenario = (variant.scenario)();

    let paths = scenario_paths(world.workspace(), variant.name);
    let _ = std::fs::remove_file(&paths.stop_file);
    let _ = std::fs::remove_dir_all(&paths.log_dir);
    world.reset_wake_dir();
    std::fs::create_dir_all(&paths.log_dir).expect("scenario worker log dir creates");

    let config = runner_config();
    let poll_profile = WorkerPollProfile {
        long_poll_ms: LONG_POLL_MS,
        ci_status_poll_ms: CI_STATUS_POLL_MS,
        ci_status_roles: variant.ci_status_poll_roles,
    };
    let spawn_start = Instant::now();
    let mut workers = WorkerFleet::spawn(
        world.server(),
        &primary,
        &repo_args,
        &paths.stop_file,
        world.wake_dir(),
        world.wake_secret(),
        &paths.log_dir,
        &paths.worker_root_dir,
        poll_profile,
        &config,
        variant.architect,
        variant.reviewer,
        variant.ci_sentinel,
    );
    timing.worker_spawn = spawn_start.elapsed();

    let ready_start = Instant::now();
    workers.wait_for_wake_sockets(Duration::from_secs(30));
    timing.worker_ready = ready_start.elapsed();

    let seed_start = Instant::now();
    seed(world.server(), &primary, &scenario, variant.name);
    timing.seed = seed_start.elapsed();

    let timeout = convergence_timeout();
    let convergence_start = Instant::now();
    let converged = poll_until_converged(world.server(), &primary, &scenario, timeout);
    timing.convergence = convergence_start.elapsed();
    if converged.is_ok() {
        assert!(
            timing.convergence < Duration::from_millis(LONG_POLL_MS),
            "scenario '{}' converged in {:?}, which is not before the {}ms non-CI poll backstop\n--- worker logs ---\n{}",
            variant.name,
            timing.convergence,
            LONG_POLL_MS,
            workers.logs()
        );
        assert!(
            workers.logs().contains("consumed authenticated wake"),
            "scenario '{}' converged without any authenticated wake in worker logs\n{}",
            variant.name,
            workers.logs()
        );
        let unexpected_polls = unexpected_poll_triggers(&workers, variant.ci_status_poll_roles);
        assert!(
            unexpected_polls.is_empty(),
            "scenario '{}' had poll-trigger ticks outside mechanical landing and the narrow CI status fallback roles {:?}:\n{}\n--- worker logs ---\n{}",
            variant.name,
            variant.ci_status_poll_roles,
            unexpected_polls.join("\n"),
            workers.logs()
        );
    }

    let teardown_start = Instant::now();
    touch(&paths.stop_file);
    let exits = workers.wait_all();
    timing.worker_teardown = teardown_start.elapsed();
    timing.total = scenario_start.elapsed();

    if let Err(error) = converged {
        let runner_running = world.runner_running();
        let runner_log = world.runner_log_tail();
        panic!(
            "scenario '{}' did not converge within {timeout:?}: {error}\n\
             timing: {}\n\
             runner running={runner_running}, runner log tail:\n{runner_log}\n\
             --- worker scan summary ---\n{}\n\
             --- worker logs ---\n{}\n\
             --- CI diagnostics ---\n{}",
            variant.name,
            timing.render(),
            workers.scan_summary(),
            workers.logs(),
            ci_diagnostics(world.server(), &repos)
        );
    }

    for (label, status) in &exits {
        assert!(
            status.success(),
            "scenario '{}' worker '{label}' exited with {status:?}, expected success\n\
             timing: {}\n--- worker logs ---\n{}",
            variant.name,
            timing.render(),
            workers.logs()
        );
    }

    timing.total = scenario_start.elapsed();
    eprintln!(
        "forgejo_multiprocess scenario '{}' scan summary:\n{}",
        variant.name,
        workers.scan_summary()
    );

    let _ = std::fs::remove_file(&paths.stop_file);
    if !std::thread::panicking() {
        let _ = std::fs::remove_dir_all(&paths.log_dir);
    }
    drop(workers);
    timing
}

fn unexpected_poll_triggers(workers: &WorkerFleet, allowed_roles: &[&str]) -> Vec<String> {
    let allowed_labels = allowed_roles
        .iter()
        .map(|role| format!("role:{role}"))
        .collect::<Vec<_>>();
    workers
        .poll_trigger_lines()
        .into_iter()
        .filter(|line| {
            !line.starts_with("mechanical:")
                && !allowed_labels
                    .iter()
                    .any(|label| line.starts_with(&format!("{label}:")))
        })
        .collect()
}

struct ScenarioLiveWorld {
    // Drop runner before server so the daemon cannot keep polling a dead Forgejo.
    runner: ForgejoRunner,
    server: ForgejoServer,
    state: ProvisionedRepositories,
    trigger_addr: SocketAddr,
    _trigger: TriggerServer,
    webhook_secret: String,
    wake_secret_file: PathBuf,
    wake_dir: PathBuf,
    workspace: RunWorkspace,
    timing: WorldTiming,
}

impl ScenarioLiveWorld {
    fn start(repo_names: &[String]) -> Self {
        let total_start = Instant::now();
        let server_start = Instant::now();
        let cached = start_cached_provisioned_repositories(repo_names)
            .expect("forgejo cached provisioned scenario state starts");
        let cache_hit = cached.cache_hit;
        let server = cached.server;
        let state = cached.state;
        let server_startup = server_start.elapsed();

        let runner_start = Instant::now();
        let mut runner = ForgejoRunner::register(&server).expect("forgejo runner registers");
        let runner_startup = runner_start.elapsed();
        assert!(runner.is_running(), "runner daemon exited immediately");

        let trigger_start = Instant::now();
        let workspace = RunWorkspace::new("temper-forgejo-multiprocess");
        let trigger_paths = TriggerPaths::new(workspace.path());
        trigger_paths.write_secrets();
        let trigger = TriggerServer::start(
            trigger_paths.webhook_secret_file.clone(),
            Some(trigger_paths.wake_secret_file.clone()),
            trigger_paths.wake_dir.clone(),
        );
        let trigger_addr = trigger.addr();
        let trigger_startup = trigger_start.elapsed();

        let timing = WorldTiming {
            server_startup,
            runner_startup,
            cache_hit,
            trigger_startup,
            total: total_start.elapsed(),
        };
        Self {
            runner,
            server,
            state,
            trigger_addr,
            _trigger: trigger,
            webhook_secret: trigger_paths.webhook_secret,
            wake_secret_file: trigger_paths.wake_secret_file,
            wake_dir: trigger_paths.wake_dir,
            workspace,
            timing,
        }
    }

    fn provision_repo(&self, name: &str) -> Provisioned {
        let provisioned = self.state.provisioned(name).unwrap_or_else(|| {
            panic!(
                "scenario cached Forgejo state did not contain repo {}/{name}",
                self.state.roles.owner
            )
        });
        register_webhook(
            &self.server,
            &provisioned.admin_token,
            &provisioned.owner,
            &provisioned.name,
            self.trigger_addr,
            &self.webhook_secret,
        );
        provisioned
    }

    fn wake_dir(&self) -> &Path {
        &self.wake_dir
    }

    fn wake_secret(&self) -> &Path {
        &self.wake_secret_file
    }

    fn reset_wake_dir(&self) {
        let _ = std::fs::remove_dir_all(&self.wake_dir);
        std::fs::create_dir_all(&self.wake_dir).expect("scenario wake dir creates");
    }

    fn server(&self) -> &ForgejoServer {
        &self.server
    }

    fn workspace(&self) -> &RunWorkspace {
        &self.workspace
    }

    fn runner_running(&mut self) -> bool {
        self.runner.is_running()
    }

    fn runner_log_tail(&self) -> String {
        self.runner.log_tail()
    }
}

#[derive(Clone, Default)]
struct WorldTiming {
    server_startup: Duration,
    runner_startup: Duration,
    cache_hit: bool,
    trigger_startup: Duration,
    total: Duration,
}

impl WorldTiming {
    fn render(&self) -> String {
        format!(
            "state_cache_hit={} server_startup={:?} runner_startup={:?} \
             trigger_startup={:?} total={:?}",
            self.cache_hit,
            self.server_startup,
            self.runner_startup,
            self.trigger_startup,
            self.total
        )
    }
}

struct TriggerPaths {
    webhook_secret: String,
    webhook_secret_file: PathBuf,
    wake_secret_file: PathBuf,
    wake_dir: PathBuf,
}

impl TriggerPaths {
    fn new(workspace: &Path) -> Self {
        let run_dir = workspace.join("trigger");
        Self {
            webhook_secret: "webhook-secret".to_string(),
            webhook_secret_file: run_dir.join("webhook-secret"),
            wake_secret_file: run_dir.join("wake-secret"),
            wake_dir: run_dir.join("wake"),
        }
    }

    fn write_secrets(&self) {
        if let Some(parent) = self.webhook_secret_file.parent() {
            std::fs::create_dir_all(parent).expect("trigger secret dir creates");
        }
        std::fs::write(
            &self.webhook_secret_file,
            format!("{}\n", self.webhook_secret),
        )
        .expect("webhook secret is written");
        std::fs::write(&self.wake_secret_file, "wake-secret\n").expect("wake secret is written");
        std::fs::create_dir_all(&self.wake_dir).expect("wake dir creates");
    }
}

fn register_webhook(
    server: &ForgejoServer,
    admin_token: &str,
    owner: &str,
    name: &str,
    addr: SocketAddr,
    secret: &str,
) {
    let url = format!("http://{addr}/forgejo/webhook");
    futures_block_on(async {
        temper_production::forgejo_rest::ensure_repo_webhook(
            &temper_production::forgejo_rest::http_client().expect("HTTP client builds"),
            server.base_url(),
            admin_token,
            owner,
            name,
            &url,
            secret,
        )
        .await
    })
    .unwrap_or_else(|error| panic!("repo webhook registration failed for {owner}/{name}: {error}"));
}

#[derive(Clone, Default)]
struct ScenarioTiming {
    repo_provision: Duration,
    seed: Duration,
    worker_spawn: Duration,
    worker_ready: Duration,
    convergence: Duration,
    worker_teardown: Duration,
    total: Duration,
}

impl ScenarioTiming {
    fn render(&self) -> String {
        format!(
            "repo_provision={:?} worker_spawn={:?} worker_ready={:?} seed={:?} \
             convergence={:?} worker_teardown={:?} total={:?}",
            self.repo_provision,
            self.worker_spawn,
            self.worker_ready,
            self.seed,
            self.convergence,
            self.worker_teardown,
            self.total
        )
    }
}

struct ScenarioPaths {
    stop_file: PathBuf,
    log_dir: PathBuf,
    worker_root_dir: PathBuf,
}

fn scenario_paths(workspace: &RunWorkspace, name: &str) -> ScenarioPaths {
    let scenario_dir = workspace.dir(Path::new("scenarios").join(name));
    ScenarioPaths {
        stop_file: scenario_dir.join("stop"),
        log_dir: scenario_dir.join("logs"),
        worker_root_dir: scenario_dir.join("worker-roots"),
    }
}

/// Builds an admin-owner [`ForgejoForge`] (admin token, default repo + web-UI
/// credentials) for seeding and asserting against the provisioned repo.
fn admin_forge(server: &ForgejoServer, provisioned: &Provisioned) -> ForgejoForge {
    let config = ForgejoConfig::new(server.base_url(), &provisioned.admin_token)
        .with_default_repo(&provisioned.owner, &provisioned.name);
    let config = match provisioned.roles.values().next() {
        Some(role) => config.with_web_ui_credentials(&role.user, &role.password),
        None => config,
    };
    ForgejoForge::new(config)
}

/// Lists each PR, its head branch, and its CI jobs for convergence-failure
/// messages. Best-effort: any read error is folded into the string.
fn ci_diagnostics(server: &ForgejoServer, repos: &[Provisioned]) -> String {
    use temper_forge::{CiJobQuery, PullRequestQuery};
    futures_block_on(async {
        let mut out = String::new();
        for provisioned in repos {
            out.push_str(&format!(
                "repo {}/{}\n",
                provisioned.owner, provisioned.name
            ));
            let forge = admin_forge(server, provisioned);
            match forge
                .list_pull_requests(&provisioned.repository, PullRequestQuery::default())
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
                        match forge.list_pull_request_reviews(&pr.id).await {
                            Ok(reviews) => {
                                for r in reviews {
                                    out.push_str(&format!(
                                        "    review by {} decision={:?} at={}\n",
                                        r.reviewer_id.as_str(),
                                        r.decision,
                                        r.submitted_at
                                    ));
                                }
                            }
                            Err(error) => {
                                out.push_str(&format!("    list reviews error: {error}\n"));
                            }
                        }
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
                                        "    job {} status={:?} conclusion={:?} created={}\n",
                                        job.name, job.status, job.conclusion, job.created_at
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

/// Seeds the provisioned repo in-process using the scenario's exact seed closure.
fn seed(server: &ForgejoServer, provisioned: &Provisioned, scenario: &Scenario, label: &str) {
    let forge = admin_forge(server, provisioned);
    futures_block_on((scenario.seed)(&forge, &provisioned.repository)).unwrap_or_else(|error| {
        panic!(
            "scenario '{label}' seeding failed for {}/{}: {error}",
            provisioned.owner, provisioned.name
        )
    });
}

/// Polls the scenario assert until it passes or the timeout elapses.
fn poll_until_converged(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    scenario: &Scenario,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let forge = admin_forge(server, provisioned);
        let last_error = match futures_block_on((scenario.assert)(&forge, &provisioned.repository))
        {
            Ok(()) => return Ok(()),
            Err(error) => error.to_string(),
        };
        if Instant::now() >= deadline {
            return Err(last_error);
        }
        std::thread::sleep(ASSERT_POLL);
    }
}

/// Drives a single boxed future to completion on a one-shot current-thread
/// runtime. The real backend's futures park on network IO, so the crate's no-op
/// `block_on` cannot drive them.
fn futures_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds")
        .block_on(future)
}

fn touch(path: &Path) {
    std::fs::write(path, b"stop").expect("writing the stop sentinel succeeds");
}
