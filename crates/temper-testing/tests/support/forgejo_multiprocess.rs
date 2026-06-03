use std::path::Path;
use std::process::{Child, Command};
use std::time::Duration;
use temper_forge::Forge;
use temper_runner::RunnerConfig;
use temper_testing::agents::fake_registry;
use temper_testing::forgejo_server::{ForgejoServer, Provisioned};
use temper_testing::worker_bin::{FORGEJO_PASSWORD_ENV, FORGEJO_TOKEN_ENV, FORGEJO_USERNAME_ENV};
use temper_testing::workflow;
use temper_workflow::RoleId;

const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(300);

/// Returns whether the env opt-in is present; prints a skip note when not.
pub fn enabled() -> bool {
    let e2e = std::env::var("TEMPER_FORGEJO_E2E").ok().as_deref() == Some("1");
    if e2e {
        return true;
    }
    eprintln!(
        "skipping Forgejo multiprocess e2e test: set TEMPER_FORGEJO_E2E=1 to enable \
         (downloads pinned Forgejo + forgejo-runner binaries and spawns a host-mode runner)"
    );
    false
}

pub fn convergence_timeout() -> Duration {
    CONVERGENCE_TIMEOUT
}

/// Owns every spawned worker process and kills any survivors on drop.
pub struct WorkerFleet {
    workers: Vec<SpawnedWorker>,
}

struct SpawnedWorker {
    label: String,
    child: Child,
}

impl WorkerFleet {
    /// Spawns one `--backend forgejo` process per role-with-an-agent, plus the
    /// mechanical worker. No CI worker — the real runner is the CI producer.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        server: &ForgejoServer,
        provisioned: &Provisioned,
        repos: &[String],
        stop_file: &Path,
        config: &RunnerConfig,
        architect: &str,
        reviewer: &str,
        ci_sentinel: &str,
    ) -> Self {
        let base = server.base_url().to_string();
        let mut workers = Vec::new();

        for role in role_workers(config) {
            let identity = provisioned
                .role(&RoleId::new(&role))
                .unwrap_or_else(|| panic!("role '{role}' is provisioned with an identity"));
            let env: Vec<(&str, &str)> = vec![
                (FORGEJO_TOKEN_ENV, identity.token.as_str()),
                (FORGEJO_USERNAME_ENV, identity.user.as_str()),
                (FORGEJO_PASSWORD_ENV, identity.password.as_str()),
            ];
            let child = spawn_worker(
                &base,
                repos,
                stop_file,
                &[
                    ("--kind", "role"),
                    ("--role", &role),
                    ("--user", &identity.user),
                    ("--architect", architect),
                    ("--reviewer", reviewer),
                    ("--ci-sentinel", ci_sentinel),
                    ("--agents", "fake"),
                ],
                &env,
            );
            workers.push(SpawnedWorker {
                label: format!("role:{role}"),
                child,
            });
        }

        let child = spawn_worker(
            &base,
            repos,
            stop_file,
            &[("--kind", "mechanical")],
            &[(FORGEJO_TOKEN_ENV, provisioned.admin_token.as_str())],
        );
        workers.push(SpawnedWorker {
            label: "mechanical".into(),
            child,
        });

        Self { workers }
    }

    /// Waits for every child and returns its (label, exit status).
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
}

impl Drop for WorkerFleet {
    fn drop(&mut self) {
        for worker in &mut self.workers {
            let _ = worker.child.kill();
            let _ = worker.child.wait();
        }
    }
}

/// Role ids that have both a registered fake agent and a configured binding —
/// the same derivation the filesystem multiprocess test uses.
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

/// Spawns the worker binary with the Forgejo backend flags and per-child env.
fn spawn_worker(
    base_url: &str,
    repos: &[String],
    stop_file: &Path,
    extra: &[(&str, &str)],
    env: &[(&str, &str)],
) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_temper-testing-worker"));
    command
        .arg("--backend")
        .arg("forgejo")
        .arg("--base-url")
        .arg(base_url);
    for repo in repos {
        command.arg("--repo").arg(repo);
    }
    command
        .arg("--root")
        .arg(std::env::temp_dir().join("temper-forgejo-mp-unused"))
        .arg("--clock")
        .arg("wall")
        .arg("--poll-ms")
        .arg(super::WORKER_POLL_MS.to_string())
        .arg("--stop-file")
        .arg(stop_file)
        .arg("--run-secs")
        .arg(super::WORKER_RUN_SECS.to_string());
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
        .spawn()
        .unwrap_or_else(|error| panic!("spawning worker {extra:?} failed: {error}"))
}
