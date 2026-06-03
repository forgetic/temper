//! Multi-repo process e2e: one fixed fake worker set scans two filesystem repos.
//!
//! This is the Phase 4 regression for `plans/multi-repo-workers/`: the driver
//! provisions two repositories in one shared `FilesystemForge` store, starts one
//! role worker per role, one mechanical worker, and one fake CI producer, and
//! passes **both** `--repo` values to every child. There is no per-repo role or
//! mechanical worker pool.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};
use temper_forge::{Forge, RepositoryId, RepositoryPath};
use temper_forge_filesystem::FilesystemForge;
use temper_runner::{RunnerConfig, Scenario};
use temper_testing::agents::fake_registry;
use temper_testing::scenarios::happy_path;
use temper_testing::worker_bin::{self, WorkerArgs, WorkerKind};
use temper_testing::{block_on, runner_config, workflow};

const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(45);
const ASSERT_POLL: Duration = Duration::from_millis(100);
const WORKER_POLL_MS: u64 = 20;
const WORKER_RUN_SECS: u64 = 120;

#[test]
#[ignore = "spawns OS processes and polls on wall-clock time; run with --ignored"]
fn one_fixed_worker_set_converges_two_filesystem_repos() {
    let root = TempRoot::new("multi-repo-multiprocess");
    let config = runner_config();
    let owner = config.repository.owner.clone();
    let paths = vec![
        RepositoryPath::new(&owner, "service-alpha"),
        RepositoryPath::new(&owner, "service-beta"),
    ];

    provision(&root.path, &paths);
    let repos = resolve_repos(&root.path, &paths);
    let scenario = happy_path();
    for repo in &repos {
        seed(&root.path, repo, &scenario);
    }

    let stop_file = root.path.join("STOP");
    let mut workers = WorkerFleet::spawn(&root.path, &paths, &stop_file, &config);
    let converged = poll_until_all_converged(&root.path, &paths, &repos, &scenario);

    touch(&stop_file);
    let exits = workers.wait_all();

    if let Err(error) = converged {
        panic!("multi-repo filesystem process world did not converge: {error}");
    }
    for (label, status) in &exits {
        assert!(status.success(), "worker '{label}' exited with {status:?}");
    }
    for (path, repo) in paths.iter().zip(repos.iter()) {
        let forge = FilesystemForge::new(&root.path);
        block_on((scenario.assert)(&forge, repo)).unwrap_or_else(|error| {
            panic!("final assertion failed for {}: {error}", display(path))
        });
    }
}

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
        stop_file: None,
        run_secs: Some(0),
        clock: worker_bin::ClockKind::Deterministic,
        agents: worker_bin::AgentsKind::Fake,
        wake_socket: None,
        wake_secret_file: None,
    };
    worker_bin::run(&args).expect("multi-repo provisioning succeeds");
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

fn seed(root: &Path, repo: &RepositoryId, scenario: &Scenario) {
    let forge = FilesystemForge::new(root);
    block_on((scenario.seed)(&forge, repo)).expect("seeding the scenario succeeds");
}

fn poll_until_all_converged(
    root: &Path,
    paths: &[RepositoryPath],
    repos: &[RepositoryId],
    scenario: &Scenario,
) -> Result<(), String> {
    let deadline = Instant::now() + CONVERGENCE_TIMEOUT;
    let mut last = vec![String::new(); repos.len()];
    loop {
        let mut all_ok = true;
        for (idx, repo) in repos.iter().enumerate() {
            let forge = FilesystemForge::new(root);
            match block_on((scenario.assert)(&forge, repo)) {
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
            return Err(paths
                .iter()
                .zip(last.iter())
                .map(|(path, error)| format!("{} => {error}", display(path)))
                .collect::<Vec<_>>()
                .join("\n"));
        }
        sleep(ASSERT_POLL);
    }
}

struct SpawnedWorker {
    label: String,
    child: Child,
}

struct WorkerFleet {
    workers: Vec<SpawnedWorker>,
}

impl WorkerFleet {
    fn spawn(
        root: &Path,
        repos: &[RepositoryPath],
        stop_file: &Path,
        config: &RunnerConfig,
    ) -> Self {
        let mut workers = Vec::new();
        let mut spawn = |label: String, extra: &[(&str, &str)]| {
            let child = spawn_worker(root, repos, stop_file, extra);
            workers.push(SpawnedWorker { label, child });
        };
        for (role, user) in role_workers(config) {
            spawn(
                format!("role:{role}"),
                &[("--kind", "role"), ("--role", &role), ("--user", &user)],
            );
        }
        spawn("mechanical".into(), &[("--kind", "mechanical")]);
        spawn("ci".into(), &[("--kind", "ci"), ("--ci", "pass")]);
        Self { workers }
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
}

impl Drop for WorkerFleet {
    fn drop(&mut self) {
        for worker in &mut self.workers {
            let _ = worker.child.kill();
            let _ = worker.child.wait();
        }
    }
}

fn role_workers(config: &RunnerConfig) -> Vec<(String, String)> {
    let compiled = workflow().compile();
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

fn spawn_worker(
    root: &Path,
    repos: &[RepositoryPath],
    stop_file: &Path,
    extra: &[(&str, &str)],
) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_temper-testing-worker"));
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

fn touch(path: &Path) {
    std::fs::write(path, b"stop").expect("writing the stop sentinel succeeds");
}

fn display(path: &RepositoryPath) -> String {
    format!("{}/{}", path.owner, path.name)
}

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
