//! True one-process-per-part multi-process rehearsal of the reference-delivery
//! workflow, across all four scenarios (Phases 4–5 of
//! `docs/explanation/multiprocess-e2e-roadmap.md`).
//!
//! Unlike the in-process `MultiProcessStage` sketch — which still runs every
//! worker in one OS process — these tests spawn the `harness-testing-worker`
//! binary once per moving part: each role-with-an-agent, the mechanical worker,
//! and the fake CI producer. They coordinate **only** through a shared
//! `FilesystemForge` store on disk. The driver itself touches the store in-process
//! only to provision, seed, and assert — never to advance workflow state.
//!
//! Each scenario reuses the exact in-process seed/assert closures from
//! `harness_testing::scenarios`; only the spawned worker *behavior* differs,
//! selected through `harness-testing-worker` flags (`--architect`, `--reviewer`,
//! `--ci`) that mirror the in-process registry wiring in
//! `harness-runner/tests/end_to_end.rs`. So the multi-process world is checked
//! against the same end state as the deterministic scenarios — no forked
//! assertion logic per topology.
//!
//! They are `#[ignore]`d on purpose: they spawn real processes and detect
//! convergence by wall-clock polling, so they are the one intentional
//! non-deterministic test, matching the env-gated Forgejo live-test precedent.
//! The deterministic in-process scenarios remain the default coverage. Run them
//! with:
//!
//! ```sh
//! cargo test -p harness-testing --test multiprocess -- --ignored
//! ```

use harness_forge::{Forge, RepositoryId, RepositoryPath};
use harness_forge_filesystem::FilesystemForge;
use harness_runner::{RunnerConfig, Scenario};
use harness_testing::agents::fake_registry;
use harness_testing::scenarios::{
    changes_requested_then_approved, ci_fails_then_passes, dependency_chain_mechanically_unblocked,
    happy_path,
};
use harness_testing::worker_bin::{self, WorkerArgs, WorkerKind};
use harness_testing::{block_on, runner_config, workflow};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// How long to wait for the multi-process world to converge before failing.
const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the driver re-runs the assert closure while polling for convergence.
const ASSERT_POLL: Duration = Duration::from_millis(100);
/// Worker poll cadence; short so the rehearsal converges quickly.
const WORKER_POLL_MS: u64 = 20;
/// Backstop run length for each child, in case the driver dies before stopping it.
const WORKER_RUN_SECS: u64 = 120;

/// The worker behavior flags that distinguish one scenario's topology.
///
/// These map one-to-one onto the in-process registry/CI wiring in
/// `harness-runner/tests/end_to_end.rs`. Only the spawned worker behavior
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
}

#[test]
#[ignore = "spawns OS processes and polls on wall-clock time; run with --ignored"]
fn happy_path_converges_across_real_processes() {
    run_variant(&Variant {
        suite: "multiprocess-happy-path",
        scenario: happy_path,
        architect: "default",
        reviewer: "default",
        ci: "pass",
    });
}

#[test]
#[ignore = "spawns OS processes and polls on wall-clock time; run with --ignored"]
fn changes_requested_then_approved_converges_across_real_processes() {
    run_variant(&Variant {
        suite: "multiprocess-changes-requested",
        scenario: changes_requested_then_approved,
        architect: "default",
        reviewer: "request-changes-then-approve",
        ci: "pass",
    });
}

#[test]
#[ignore = "spawns OS processes and polls on wall-clock time; run with --ignored"]
fn ci_fails_then_passes_converges_across_real_processes() {
    run_variant(&Variant {
        suite: "multiprocess-ci-fails",
        scenario: ci_fails_then_passes,
        architect: "default",
        reviewer: "default",
        ci: "fail-then-pass",
    });
}

#[test]
#[ignore = "spawns OS processes and polls on wall-clock time; run with --ignored"]
fn dependency_chain_mechanically_unblocked_across_real_processes() {
    run_variant(&Variant {
        suite: "multiprocess-dependency-chain",
        scenario: dependency_chain_mechanically_unblocked,
        architect: "closing",
        reviewer: "default",
        ci: "pass",
    });
}

/// Drives one scenario through the true multi-process topology and asserts it
/// converges to the same end state the in-process scenario would.
fn run_variant(variant: &Variant) {
    let root = TempRoot::new(variant.suite);
    let config = runner_config();
    let (owner, name) = (
        config.repository.owner.clone(),
        config.repository.name.clone(),
    );

    // Provision (repo + labels) in-process through the worker library, then seed
    // via the scenario's exact seed closure against the shared store. The driver
    // only ever touches the store to set up and observe; workflow state is
    // advanced exclusively by the spawned worker processes.
    provision(&root.path, &owner, &name);
    let repo = resolve_repo(&root.path, &owner, &name);
    let scenario = (variant.scenario)();
    seed(&root.path, &repo, &scenario);

    let stop_file = root.path.join("STOP");

    // Spawn one process per moving part behind a kill-on-drop guard, so a panic
    // anywhere below never orphans a child.
    let mut workers = WorkerFleet::spawn(&root.path, &owner, &name, &stop_file, &config, variant);

    // Detect convergence in-process by polling the exact scenario assert.
    let converged = poll_until_converged(&repo, &scenario, &root.path);

    // Stop: touch the sentinel and join every child.
    touch(&stop_file);
    let exits = workers.wait_all();

    // Surface a clean failure message if the world never converged.
    if let Err(error) = converged {
        panic!("multi-process world did not converge within {CONVERGENCE_TIMEOUT:?}: {error}");
    }

    // Every worker must have exited cleanly.
    for (label, status) in &exits {
        assert!(
            status.success(),
            "worker '{label}' exited with {status:?}, expected success"
        );
    }

    // Run the assert once more against the final store for a clean message.
    let forge = FilesystemForge::new(&root.path);
    let assert = (scenario.assert)(&forge, &repo);
    block_on(assert).expect("final scenario assertion passes after workers stop");
}

/// Provisions repo + labels in-process via the worker library's provision kind.
fn provision(root: &Path, owner: &str, name: &str) {
    let args = WorkerArgs {
        kind: WorkerKind::Provision,
        backend: worker_bin::Backend::Filesystem,
        root: root.to_path_buf(),
        owner: owner.to_string(),
        name: name.to_string(),
        poll_interval: chrono::Duration::milliseconds(WORKER_POLL_MS as i64),
        stop_file: None,
        run_secs: Some(0),
        clock: worker_bin::ClockKind::Deterministic,
        agents: worker_bin::AgentsKind::Fake,
        auth: worker_bin::AgentsAuthKind::default(),
        codex_model: None,
        auth_file: None,
    };
    worker_bin::run(&args).expect("provisioning the repository and labels succeeds");
}

fn resolve_repo(root: &Path, owner: &str, name: &str) -> RepositoryId {
    let forge = FilesystemForge::new(root);
    block_on(forge.get_repository_by_path(&RepositoryPath::new(owner, name)))
        .expect("repository lookup succeeds")
        .expect("provisioned repository exists")
        .id
}

/// Seeds the store in-process using the scenario's exact seed closure.
fn seed(root: &Path, repo: &RepositoryId, scenario: &Scenario) {
    let forge = FilesystemForge::new(root);
    block_on((scenario.seed)(&forge, repo)).expect("seeding the scenario succeeds");
}

/// Polls the scenario assert until it passes or the timeout elapses.
fn poll_until_converged(
    repo: &RepositoryId,
    scenario: &harness_runner::Scenario,
    root: &Path,
) -> Result<(), String> {
    let deadline = Instant::now() + CONVERGENCE_TIMEOUT;
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

/// Owns every spawned worker process and kills any survivors on drop, so a panic
/// in the driver never orphans a child.
struct WorkerFleet {
    workers: Vec<SpawnedWorker>,
}

impl WorkerFleet {
    /// Spawns one process per role-with-an-agent, plus mechanical, plus CI.
    fn spawn(
        root: &Path,
        owner: &str,
        name: &str,
        stop_file: &Path,
        config: &RunnerConfig,
        variant: &Variant,
    ) -> Self {
        let repo = format!("{owner}/{name}");
        let mut workers = Vec::new();
        let mut spawn = |label: String, extra: &[(&str, &str)]| {
            let child = spawn_worker(root, &repo, stop_file, extra);
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

    /// Waits for every child and returns its (label, exit status).
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

fn spawn_worker(root: &Path, repo: &str, stop_file: &Path, extra: &[(&str, &str)]) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_harness-testing-worker"));
    command
        .arg("--root")
        .arg(root)
        .arg("--repo")
        .arg(repo)
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
            "harness-testing-{suite}-{}-{id}",
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
