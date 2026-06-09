//! True one-process-per-part multi-process rehearsal of the reference-delivery
//! workflow, across all five scenarios documented in
//! `docs/how-to/run-multiprocess-e2e.md`.
//!
//! Unlike the in-process `MultiProcessStage` sketch — which still runs every
//! worker in one OS process — these tests spawn the `temper-testing-worker`
//! binary once per moving part: each role-with-an-agent, the mechanical worker,
//! and the fake CI producer. They coordinate **only** through a shared
//! `FilesystemForge` store on disk. The driver itself touches the store in-process
//! only to provision, seed, and assert — never to advance workflow state.
//!
//! Each scenario reuses the exact in-process seed/assert closures from
//! `temper_testing::scenarios`; only the spawned worker *behavior* differs,
//! selected through `temper-testing-worker` flags (`--architect`, `--reviewer`,
//! `--ci`) that mirror the in-process registry wiring in
//! `temper-runner/tests/end_to_end.rs`. So the multi-process world is checked
//! against the same end state as the deterministic scenarios — no forked
//! assertion logic per topology.
//!
//! These filesystem-backed process-boundary tests are part of the default suite:
//! they spawn real processes and detect convergence by wall-clock polling, but
//! they are fast enough to run on every `cargo dev-test-quick`. The deterministic
//! in-process scenarios remain the first-line coverage; this file adds the
//! process-boundary regression.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};
use temper_forge::{Forge, RepositoryId, RepositoryPath};
use temper_forge_filesystem::FilesystemForge;
use temper_runner::{RunnerConfig, Scenario};
use temper_testing::agents::fake_registry;
use temper_testing::scenarios::{
    changes_requested_then_approved, ci_fails_then_passes, cross_repo_fanout_converges,
    dependency_chain_mechanically_unblocked, happy_path,
};
use temper_testing::worker_bin::{self, WorkerArgs, WorkerKind};
use temper_testing::{block_on, runner_config, workflow};

#[path = "support/worker_binary.rs"]
mod worker_binary;

/// How long to wait for the multi-process world to converge before failing.
fn convergence_timeout() -> Duration {
    const DEFAULT_SECS: u64 = 120;
    let secs = std::env::var("TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SECS);
    Duration::from_secs(secs)
}
/// How often the driver re-runs the assert closure while polling for convergence.
const ASSERT_POLL: Duration = Duration::from_millis(100);
/// Worker poll cadence; short so the rehearsal converges quickly.
const WORKER_POLL_MS: u64 = 20;
/// Backstop run length for each child, in case the driver dies before stopping it.
const WORKER_RUN_SECS: u64 = 120;
/// How long to wait for workers to observe the stop sentinel before killing them.
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// Serializes these tests when Rust's test harness runs them in parallel.
static MULTIPROCESS_E2E_LOCK: Mutex<()> = Mutex::new(());

/// The worker behavior flags that distinguish one scenario's topology.
///
/// These map one-to-one onto the in-process registry/CI wiring in
/// `temper-runner/tests/end_to_end.rs`. Only the spawned worker behavior
/// changes per scenario; the seed/assert closures are reused verbatim.
struct Variant {
    /// Suite name used for the temp-store directory.
    suite: &'static str,
    /// The scenario whose seed/assert closures the driver reuses.
    scenario: fn() -> Scenario,
    /// `--architect` value passed to the architect role worker.
    architect: &'static str,
    /// `--reviewer` value passed to the reviewer role worker.
    reviewer: &'static str,
    /// `--ci` policy passed to the CI producer process.
    ci: &'static str,
    /// Whether this variant runs one fixed worker fleet over two repositories.
    multi_repo: bool,
}

#[test]
fn happy_path_converges_across_real_processes() {
    run_variant(&Variant {
        suite: "multiprocess-happy-path",
        scenario: happy_path,
        architect: "default",
        reviewer: "default",
        ci: "pass",
        multi_repo: false,
    });
}

#[test]
fn changes_requested_then_approved_converges_across_real_processes() {
    run_variant(&Variant {
        suite: "multiprocess-changes-requested",
        scenario: changes_requested_then_approved,
        architect: "default",
        reviewer: "request-changes-then-approve",
        ci: "pass",
        multi_repo: false,
    });
}

#[test]
fn ci_fails_then_passes_converges_across_real_processes() {
    run_variant(&Variant {
        suite: "multiprocess-ci-fails",
        scenario: ci_fails_then_passes,
        architect: "default",
        reviewer: "default",
        ci: "fail-then-pass",
        multi_repo: false,
    });
}

#[test]
fn dependency_chain_mechanically_unblocked_across_real_processes() {
    run_variant(&Variant {
        suite: "multiprocess-dependency-chain",
        scenario: dependency_chain_mechanically_unblocked,
        architect: "closing",
        reviewer: "default",
        ci: "pass",
        multi_repo: false,
    });
}

#[test]
fn cross_repo_fanout_converges_across_one_fixed_worker_fleet() {
    run_variant(&Variant {
        suite: "multiprocess-cross-repo-fanout",
        scenario: cross_repo_fanout_converges,
        architect: "closing",
        reviewer: "default",
        ci: "pass",
        multi_repo: true,
    });
}

/// Drives one scenario through the true multi-process topology and asserts it
/// converges to the same end state the in-process scenario would.
fn run_variant(variant: &Variant) {
    // The Rust test harness runs tests in parallel by default. Each scenario
    // launches a whole worker fleet; running all fleets at once
    // makes this wall-clock topology rehearsal oversubscribe the host and can
    // leave a child slow to observe shutdown. The process-boundary property is
    // covered within each fleet, so serialize scenarios inside this test binary.
    let _guard = MULTIPROCESS_E2E_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let root = TempRoot::new(variant.suite);
    let config = runner_config();
    let paths = scenario_paths(&config, variant.multi_repo);

    // Provision (repo + labels) in-process through the worker library, then seed
    // via the scenario's exact seed closure against the shared store. The driver
    // only ever touches the store to set up and observe; workflow state is
    // advanced exclusively by the spawned worker processes.
    provision(&root.path, &paths);
    let repos = resolve_repos(&root.path, &paths);
    let repo = repos[0].clone();
    let scenario = (variant.scenario)();
    seed(&root.path, &repo, &scenario);

    let stop_file = root.path.join("STOP");

    // Spawn one process per moving part behind a kill-on-drop guard, so a panic
    // anywhere below never orphans a child.
    let mut workers = WorkerFleet::spawn(&root.path, &paths, &stop_file, &config, variant);

    // Detect convergence in-process by polling the exact scenario assert.
    let timeout = convergence_timeout();
    let converged = poll_until_converged(&repo, &scenario, &root.path, timeout);

    // Stop: touch the sentinel and join every child.
    touch(&stop_file);
    let exits = workers.wait_all();

    // Surface a clean failure message if the world never converged.
    if let Err(error) = converged {
        panic!("multi-process world did not converge within {timeout:?}: {error}");
    }

    // Every worker must have exited cleanly.
    for exit in &exits {
        assert!(
            !exit.timed_out && exit.status.success(),
            "worker '{}' {}exited with {:?}, expected success",
            exit.label,
            if exit.timed_out {
                "did not stop within the shutdown timeout; "
            } else {
                ""
            },
            exit.status,
        );
    }

    // Run the assert once more against the final store for a clean message.
    let forge = FilesystemForge::new(&root.path);
    let assert = (scenario.assert)(&forge, &repo);
    block_on(assert).expect("final scenario assertion passes after workers stop");
}

fn scenario_paths(config: &RunnerConfig, multi_repo: bool) -> Vec<RepositoryPath> {
    let mut paths = vec![RepositoryPath::new(
        &config.repository.owner,
        &config.repository.name,
    )];
    if multi_repo {
        paths.push(RepositoryPath::new(
            &config.repository.owner,
            "service-canary",
        ));
    }
    paths
}

/// Provisions repo + labels in-process via the worker library's provision kind.
fn provision(root: &Path, paths: &[RepositoryPath]) {
    let first = &paths[0];
    let args = WorkerArgs {
        kind: WorkerKind::Provision,
        backend: worker_bin::Backend::Filesystem,
        root: root.to_path_buf(),
        owner: first.owner.clone(),
        name: first.name.clone(),
        repositories: paths.to_vec(),
        poll_interval: chrono::Duration::milliseconds(WORKER_POLL_MS as i64),
        idle_poll_max_interval: chrono::Duration::milliseconds(60_000),
        audit_interval: Some(chrono::Duration::milliseconds(600_000)),
        stop_file: None,
        run_secs: Some(0),
        clock: worker_bin::ClockKind::Deterministic,
        agents: worker_bin::AgentsKind::Fake,
        wake_socket: None,
        wake_secret_file: None,
        workflow_file: None,
    };
    worker_bin::run(&args).expect("provisioning the repositories and labels succeeds");
}

fn resolve_repos(root: &Path, paths: &[RepositoryPath]) -> Vec<RepositoryId> {
    let forge = FilesystemForge::new(root);
    paths
        .iter()
        .map(|path| {
            block_on(forge.get_repository_by_path(path))
                .unwrap_or_else(|error| {
                    panic!("repository lookup failed for {}: {error}", display(path))
                })
                .unwrap_or_else(|| panic!("provisioned repository {} exists", display(path)))
                .id
        })
        .collect()
}

/// Seeds the store in-process using the scenario's exact seed closure.
fn seed(root: &Path, repo: &RepositoryId, scenario: &Scenario) {
    let forge = FilesystemForge::new(root);
    block_on((scenario.seed)(&forge, repo)).expect("seeding the scenario succeeds");
}

/// Polls the scenario assert until it passes or the timeout elapses.
fn poll_until_converged(
    repo: &RepositoryId,
    scenario: &temper_runner::Scenario,
    root: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let forge = FilesystemForge::new(root);
        let last_error = match block_on((scenario.assert)(&forge, repo)) {
            Ok(()) => return Ok(()),
            Err(error) => error.to_string(),
        };
        if Instant::now() >= deadline {
            return Err(last_error);
        }
        sleep(ASSERT_POLL);
    }
}

fn touch(path: &Path) {
    std::fs::write(path, b"stop").expect("writing the stop sentinel succeeds");
}

/// A spawned worker process labelled for clear failure messages.
struct SpawnedWorker {
    label: String,
    child: Child,
}

/// A worker process's shutdown result.
struct WorkerExit {
    label: String,
    status: ExitStatus,
    timed_out: bool,
}

/// Owns every spawned worker process and kills any survivors on drop, so a panic
/// in the driver never orphans a child.
struct WorkerFleet {
    workers: Vec<SpawnedWorker>,
}

impl WorkerFleet {
    /// Spawns one process per role-with-an-agent, plus mechanical, plus CI.
    fn spawn(
        root: &Path,
        repos: &[RepositoryPath],
        stop_file: &Path,
        config: &RunnerConfig,
        variant: &Variant,
    ) -> Self {
        let mut workers = Vec::new();
        let mut spawn = |label: String, extra: &[(&str, &str)]| {
            let child = spawn_worker(root, repos, stop_file, extra);
            workers.push(SpawnedWorker { label, child });
        };

        // Derive the role workers from the compiled workflow ∩ registered agents
        // ∩ configured role bindings, never a hardcoded list. The behavior flags
        // only bite for the architect and reviewer; every other role ignores them.
        for (role, user) in role_workers(config) {
            spawn(
                format!("role:{role}"),
                &[
                    ("--kind", "role"),
                    ("--role", &role),
                    ("--user", &user),
                    ("--architect", variant.architect),
                    ("--reviewer", variant.reviewer),
                ],
            );
        }
        spawn("mechanical".into(), &[("--kind", "mechanical")]);
        spawn("ci".into(), &[("--kind", "ci"), ("--ci", variant.ci)]);

        Self { workers }
    }

    /// Waits for every child, killing survivors after a short shutdown timeout.
    fn wait_all(&mut self) -> Vec<WorkerExit> {
        let mut exits: Vec<Option<WorkerExit>> = std::iter::repeat_with(|| None)
            .take(self.workers.len())
            .collect();
        let mut remaining = self.workers.len();
        let deadline = Instant::now() + WORKER_STOP_TIMEOUT;

        while remaining > 0 && Instant::now() < deadline {
            for (index, worker) in self.workers.iter_mut().enumerate() {
                if exits[index].is_some() {
                    continue;
                }
                let Some(status) = worker.child.try_wait().unwrap_or_else(|error| {
                    panic!("polling '{}' exit failed: {error}", worker.label)
                }) else {
                    continue;
                };
                exits[index] = Some(WorkerExit {
                    label: worker.label.clone(),
                    status,
                    timed_out: false,
                });
                remaining -= 1;
            }
            if remaining > 0 {
                sleep(Duration::from_millis(10));
            }
        }

        for (index, worker) in self.workers.iter_mut().enumerate() {
            if exits[index].is_some() {
                continue;
            }
            let status = match worker.child.try_wait().unwrap_or_else(|error| {
                panic!(
                    "polling '{}' exit before kill failed: {error}",
                    worker.label
                )
            }) {
                Some(status) => status,
                None => {
                    let _ = worker.child.kill();
                    worker.child.wait().unwrap_or_else(|error| {
                        panic!("waiting on killed '{}' failed: {error}", worker.label)
                    })
                }
            };
            exits[index] = Some(WorkerExit {
                label: worker.label.clone(),
                status,
                timed_out: true,
            });
        }

        exits
            .into_iter()
            .map(|exit| exit.expect("every worker has an exit result"))
            .collect()
    }
}

impl Drop for WorkerFleet {
    fn drop(&mut self) {
        for worker in &mut self.workers {
            // Best-effort: a clean run already waited these out; this only fires
            // when the driver panicked before stopping the fleet.
            let _ = worker.child.kill();
            let _ = worker.child.wait();
        }
    }
}

/// Role ids that have both a registered fake agent and a configured binding.
fn role_workers(config: &RunnerConfig) -> Vec<(String, String)> {
    let workflow = workflow();
    let compiled = workflow.compile();
    let registry = fake_registry::<FilesystemForge>();
    compiled
        .roles()
        .iter()
        .filter(|role| registry.get(&role.id).is_some())
        .filter_map(|role| {
            config
                .role_binding(&role.id)
                .map(|binding| (role.id.to_string(), binding.user.handle.clone()))
        })
        .collect()
}

fn display(path: &RepositoryPath) -> String {
    format!("{}/{}", path.owner, path.name)
}

fn spawn_worker(
    root: &Path,
    repos: &[RepositoryPath],
    stop_file: &Path,
    extra: &[(&str, &str)],
) -> Child {
    let mut command = Command::new(worker_binary::temper_testing_worker());
    command.arg("--root").arg(root);
    for repo in repos {
        command.arg("--repo").arg(display(repo));
    }
    command
        .arg("--poll-ms")
        .arg(WORKER_POLL_MS.to_string())
        .arg("--stop-file")
        .arg(stop_file)
        .arg("--run-secs")
        .arg(WORKER_RUN_SECS.to_string());
    for (flag, value) in extra {
        command.arg(flag).arg(value);
    }
    command
        .spawn()
        .unwrap_or_else(|error| panic!("spawning worker {extra:?} failed: {error}"))
}

/// A unique temp store root cleaned up on drop.
struct TempRoot {
    path: PathBuf,
}

static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(0);

impl TempRoot {
    fn new(suite: &str) -> Self {
        let id = NEXT_TEMP_ROOT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "temper-testing-{suite}-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
