//! Forgejo multi-process scenarios against one shared live Forgejo world.
//!
//! This is the real-backend twin of `tests/multiprocess.rs`: deterministic fake
//! role agents and a mechanical worker coordinate only through a real Forgejo
//! backend while a real host-mode `forgejo-runner` produces CI. The suite keeps
//! real-backend coverage but avoids rebooting Forgejo for every scenario:
//!
//! 1. boot one [`ForgejoServer`] and register one [`ForgejoRunner`];
//! 2. bootstrap one admin and one per-role identity/token map;
//! 3. provision fresh repository names per scenario (plus one second repo only
//!    for cross-repo fan-out);
//! 4. register repo webhooks against one shared trigger;
//! 5. spawn a fresh worker fleet per scenario with unique stop file, wake
//!    sockets, and logs;
//! 6. poll the scenario's backend-neutral assert closure to convergence while
//!    the workers advance through webhook wakes, not their poll backstop.
//!
//! The five scenarios are collapsed into one ignored test so ordinary Rust
//! ownership performs panic cleanup: the active worker fleet is a stack value and
//! kills children on drop, and the shared runner/server handles are dropped if any
//! scenario panics.
//!
//! ```sh
//! cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
//! ```

#![cfg(unix)]

use std::net::{SocketAddr, TcpListener, TcpStream};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_production::trigger_args::TriggerArgs;
use temper_runner::Scenario;
use temper_testing::forgejo_server::provision::bootstrap_admin;
use temper_testing::forgejo_server::{
    provision_repository, provision_role_identities, ForgejoRunner, ForgejoServer, Provisioned,
    ProvisionedRoles,
};
#[path = "support/forgejo_multiprocess.rs"]
mod multiprocess_support;

use multiprocess_support::{convergence_timeout, WorkerFleet, WorkerPollProfile};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use temper_testing::runner_config;
use temper_testing::scenarios::{
    changes_requested_then_approved, ci_fails_then_passes, cross_repo_fanout_converges,
    dependency_chain_mechanically_unblocked, happy_path,
};

/// How often the driver re-runs the assert closure while polling.
const ASSERT_POLL: Duration = Duration::from_secs(1);
/// Deliberately long worker poll backstop; scenario progress should come from
/// Forgejo webhooks delivered through wake sockets before this deadline.
const LONG_POLL_MS: u64 = 120_000;
/// Forgejo 7.0.x does not emit Actions-completion webhooks through repository
/// hooks, so CI-reading roles keep a narrow poll backstop for CI status
/// transitions only.
const CI_STATUS_POLL_MS: u64 = 1_000;
/// Backstop run length per child, in case the driver dies before stopping it.
const WORKER_RUN_SECS: u64 = 1200;

#[derive(Clone, Copy)]
struct Variant {
    /// Stable name used in logs, repo names, and panic messages.
    name: &'static str,
    /// The scenario whose seed/assert closures the driver reuses.
    scenario: fn() -> Scenario,
    /// Fresh primary repository for this scenario.
    primary_repo: &'static str,
    /// Optional second repository; only the cross-repo fan-out scenario uses it.
    extra_repo: Option<&'static str>,
    /// `--architect` value passed to the architect role worker.
    architect: &'static str,
    /// `--reviewer` value passed to the reviewer role worker.
    reviewer: &'static str,
    /// `--ci-sentinel` value passed to the engineer role worker.
    ci_sentinel: &'static str,
    /// Role workers allowed to use the narrow CI status-poll fallback. All other
    /// workers keep the long webhook-only poll backstop.
    ci_status_poll_roles: &'static [&'static str],
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn forgejo_multiprocess_scenarios_converge_against_shared_forgejo_world() {
    let suite_start = Instant::now();
    let mut world = SharedLiveWorld::start();
    eprintln!(
        "forgejo_multiprocess shared world timing: {}",
        world.timing.render()
    );

    let mut scenario_total = Duration::ZERO;
    for variant in variants() {
        let timing = run_variant(&mut world, &variant);
        scenario_total += timing.total;
        eprintln!(
            "forgejo_multiprocess scenario '{}' timing: {}",
            variant.name,
            timing.render()
        );
    }

    let world_teardown_start = Instant::now();
    drop(world);
    let world_teardown = world_teardown_start.elapsed();
    eprintln!(
        "forgejo_multiprocess suite timing: scenarios={scenario_total:?} \
         world_teardown={world_teardown:?} total={:?}",
        suite_start.elapsed()
    );
}

fn variants() -> [Variant; 5] {
    [
        Variant {
            name: "happy_path",
            scenario: happy_path,
            primary_repo: "service-happy-path",
            extra_repo: None,
            architect: "default",
            reviewer: "default",
            ci_sentinel: "present",
            ci_status_poll_roles: &["owner"],
        },
        Variant {
            name: "changes_requested_then_approved",
            scenario: changes_requested_then_approved,
            primary_repo: "service-review-cycle",
            extra_repo: None,
            architect: "default",
            reviewer: "request-changes-then-approve",
            ci_sentinel: "present",
            ci_status_poll_roles: &["owner"],
        },
        Variant {
            name: "ci_fails_then_passes",
            scenario: ci_fails_then_passes,
            primary_repo: "service-ci-retry",
            extra_repo: None,
            architect: "default",
            reviewer: "default",
            ci_sentinel: "deferred",
            ci_status_poll_roles: &["engineer", "owner"],
        },
        Variant {
            name: "dependency_chain_mechanically_unblocked",
            scenario: dependency_chain_mechanically_unblocked,
            primary_repo: "service-dependency-chain",
            extra_repo: None,
            architect: "closing",
            reviewer: "default",
            ci_sentinel: "present",
            ci_status_poll_roles: &["owner"],
        },
        Variant {
            name: "cross_repo_fanout",
            scenario: cross_repo_fanout_converges,
            primary_repo: "service-cross-repo-source",
            // `cross_repo_targets` chooses the first visible repo other than the
            // source by owner/name, so keep the scenario's intended target before
            // the single-repo scenario names from earlier in this shared world.
            extra_repo: Some("aaa-cross-repo-target"),
            architect: "closing",
            reviewer: "default",
            ci_sentinel: "present",
            ci_status_poll_roles: &["owner"],
        },
    ]
}

/// Drives one scenario through the true multi-process topology against a real
/// Forgejo, asserting it converges to the same end state as the in-process
/// scenario.
fn run_variant(world: &mut SharedLiveWorld, variant: &Variant) -> ScenarioTiming {
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

    let paths = scenario_paths(&primary);
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
            "scenario '{}' had poll-trigger ticks outside the narrow CI status fallback roles {:?}:\n{}\n--- worker logs ---\n{}",
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
            !allowed_labels
                .iter()
                .any(|label| line.starts_with(&format!("{label}:")))
        })
        .collect()
}

struct SharedLiveWorld {
    // Drop runner before server so the daemon cannot keep polling a dead Forgejo.
    runner: ForgejoRunner,
    server: ForgejoServer,
    roles: ProvisionedRoles,
    default_branch: String,
    trigger_addr: SocketAddr,
    webhook_secret: String,
    wake_secret_file: PathBuf,
    wake_dir: PathBuf,
    timing: WorldTiming,
}

impl SharedLiveWorld {
    fn start() -> Self {
        let total_start = Instant::now();
        let server_start = Instant::now();
        let server = ForgejoServer::start().expect("forgejo server boots");
        let server_startup = server_start.elapsed();

        let runner_start = Instant::now();
        let mut runner = ForgejoRunner::register(&server).expect("forgejo runner registers");
        let runner_startup = runner_start.elapsed();
        assert!(runner.is_running(), "runner daemon exited immediately");

        let identity_start = Instant::now();
        let admin_token = bootstrap_admin(&server).expect("forgejo admin bootstrap succeeds");
        let config = runner_config();
        let roles = futures_block_on(provision_role_identities(
            server.base_url(),
            &admin_token,
            &config.repository.owner,
            &config.role_bindings,
        ))
        .expect("forgejo role identities provision once");
        let identity_provision = identity_start.elapsed();

        let trigger_start = Instant::now();
        let trigger_paths = TriggerPaths::new(server.data_dir());
        trigger_paths.write_secrets();
        let trigger_addr = free_addr();
        start_trigger(
            trigger_addr,
            trigger_paths.webhook_secret_file.clone(),
            trigger_paths.wake_secret_file.clone(),
            trigger_paths.wake_dir.clone(),
        );
        wait_for_trigger(trigger_addr);
        let trigger_startup = trigger_start.elapsed();

        let timing = WorldTiming {
            server_startup,
            runner_startup,
            identity_provision,
            trigger_startup,
            total: total_start.elapsed(),
        };
        Self {
            runner,
            server,
            roles,
            default_branch: config.repository.default_branch,
            trigger_addr,
            webhook_secret: trigger_paths.webhook_secret,
            wake_secret_file: trigger_paths.wake_secret_file,
            wake_dir: trigger_paths.wake_dir,
            timing,
        }
    }

    fn provision_repo(&self, name: &str) -> Provisioned {
        let provisioned = futures_block_on(provision_repository(
            self.server.base_url(),
            &self.roles,
            name,
            &self.default_branch,
        ))
        .unwrap_or_else(|error| {
            panic!(
                "provisioning repo {}/{} failed: {error}",
                self.roles.owner, name
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
    identity_provision: Duration,
    trigger_startup: Duration,
    total: Duration,
}

impl WorldTiming {
    fn render(&self) -> String {
        format!(
            "server_startup={:?} runner_startup={:?} identity_provision={:?} \
             trigger_startup={:?} total={:?}",
            self.server_startup,
            self.runner_startup,
            self.identity_provision,
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
    fn new(server_data_dir: &Path) -> Self {
        let run_dir = server_data_dir.join("multiprocess-webhook");
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
            eprintln!("forgejo multiprocess trigger exited: {error}");
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
}

fn scenario_paths(provisioned: &Provisioned) -> ScenarioPaths {
    let id = NEXT_SCENARIO.fetch_add(1, Ordering::SeqCst);
    let base = format!(
        "temper-forgejo-mp-{}-{}-{id}",
        std::process::id(),
        provisioned.name,
    );
    ScenarioPaths {
        stop_file: std::env::temp_dir().join(format!("{base}.stop")),
        log_dir: std::env::temp_dir().join(format!("{base}-logs")),
    }
}

static NEXT_SCENARIO: AtomicU64 = AtomicU64::new(0);

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
