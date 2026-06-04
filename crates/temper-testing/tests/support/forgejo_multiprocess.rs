use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use temper_forge::Forge;
use temper_runner::RunnerConfig;
use temper_testing::agents::fake_registry;
use temper_testing::forgejo_server::{ForgejoServer, Provisioned};
use temper_testing::worker_bin::{FORGEJO_PASSWORD_ENV, FORGEJO_TOKEN_ENV, FORGEJO_USERNAME_ENV};
use temper_testing::workflow;
use temper_workflow::RoleId;

const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(300);

pub fn convergence_timeout() -> Duration {
    CONVERGENCE_TIMEOUT
}

/// Per-worker poll cadence. CI-reading roles and the mechanical landing worker
/// can use a narrow status-poll fallback while every other worker keeps the long
/// webhook-only backstop.
#[derive(Clone, Copy)]
pub struct WorkerPollProfile {
    pub long_poll_ms: u64,
    pub ci_status_poll_ms: u64,
    pub ci_status_roles: &'static [&'static str],
}

impl WorkerPollProfile {
    fn for_role(&self, role: &str) -> u64 {
        if self.ci_status_roles.contains(&role) {
            self.ci_status_poll_ms
        } else {
            self.long_poll_ms
        }
    }

    fn for_mechanical(&self) -> u64 {
        self.ci_status_poll_ms
    }
}

/// Owns every spawned worker process and kills any survivors on drop.
pub struct WorkerFleet {
    workers: Vec<SpawnedWorker>,
}

struct SpawnedWorker {
    label: String,
    log: PathBuf,
    wake_socket: PathBuf,
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
        wake_dir: &Path,
        wake_secret: &Path,
        log_dir: &Path,
        poll_profile: WorkerPollProfile,
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
            let log = log_dir.join(format!("role-{role}.log"));
            let wake_socket = wake_dir.join(format!("{role}.sock"));
            let child = spawn_worker(
                &base,
                repos,
                stop_file,
                wake_secret,
                &wake_socket,
                poll_profile.for_role(&role),
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
                &log,
            );
            workers.push(SpawnedWorker {
                label: format!("role:{role}"),
                log,
                wake_socket,
                child,
            });
        }

        let log = log_dir.join("mechanical.log");
        let wake_socket = wake_dir.join("mechanical.sock");
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
            stop_file,
            wake_secret,
            &wake_socket,
            poll_profile.for_mechanical(),
            &[("--kind", "mechanical")],
            &mechanical_env,
            &log,
        );
        workers.push(SpawnedWorker {
            label: "mechanical".into(),
            log,
            wake_socket,
            child,
        });

        Self { workers }
    }

    /// Waits until each worker has bound its wake socket before the driver seeds
    /// webhook-generating work. The workers' initial empty scans can overlap
    /// seeding; any webhooks delivered during that scan queue on the socket and
    /// wake the next loop iteration.
    pub fn wait_for_wake_sockets(&self, timeout: Duration) {
        let deadline = std::time::Instant::now() + timeout;
        for worker in &self.workers {
            loop {
                if worker.wake_socket.exists() {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "worker '{}' did not bind wake socket {}; logs:\n{}",
                    worker.label,
                    worker.wake_socket.display(),
                    self.logs()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
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

    /// Per-worker tick/scan counters parsed from worker logs.
    pub fn scan_summary(&self) -> String {
        self.workers
            .iter()
            .map(|worker| summarize_log(&worker.label, &worker.log))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Completed poll-trigger ticks, prefixed with their worker labels.
    pub fn poll_trigger_lines(&self) -> Vec<String> {
        self.workers
            .iter()
            .flat_map(|worker| {
                std::fs::read_to_string(&worker.log)
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| line.contains("completed tick trigger=poll"))
                    .map(|line| format!("{}: {line}", worker.label))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Tails every worker log for timeout diagnostics.
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
#[allow(clippy::too_many_arguments)]
fn spawn_worker(
    base_url: &str,
    repos: &[String],
    stop_file: &Path,
    wake_secret: &Path,
    wake_socket: &Path,
    poll_ms: u64,
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
        .arg(poll_ms.to_string())
        .arg("--stop-file")
        .arg(stop_file)
        .arg("--run-secs")
        .arg(super::WORKER_RUN_SECS.to_string())
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
        .env_remove(FORGEJO_PASSWORD_ENV)
        .env("TEMPER_FORGEJO_CI_DIAGNOSTICS", "1")
        // The shared five-scenario suite has a dedicated long-poll backstop;
        // use a shorter wake drain window so webhook bursts do not dominate
        // runtime while still coalescing same-turn Forgejo hook fan-out.
        .env("TEMPER_WAKE_DEBOUNCE_MS", "50");
    for (key, value) in env {
        command.env(key, value);
    }
    command
        .stdout(Stdio::from(log.try_clone().expect("worker log clones")))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap_or_else(|error| panic!("spawning worker {extra:?} failed: {error}"))
}

fn summarize_log(label: &str, log: &Path) -> String {
    let contents = match std::fs::read_to_string(log) {
        Ok(contents) => contents,
        Err(error) => return format!("{label}: log unreadable at {}: {error}", log.display()),
    };
    let mut ticks = 0u64;
    let mut scanned_sum = 0u64;
    let mut last_paths = "-".to_string();
    let mut ci_read_lines = 0u64;
    for line in contents.lines() {
        if line.contains("completed tick") {
            ticks += 1;
            if let Some(count) = value_after(line, "scanned_repositories=")
                .and_then(|value| value.parse::<u64>().ok())
            {
                scanned_sum += count;
            }
            if let Some(paths) = value_after(line, "scanned_repository_paths=") {
                last_paths = paths.to_string();
            }
        }
        if line.contains("list_ci_jobs") || line.contains("read_ci_jobs") {
            ci_read_lines += 1;
        }
    }
    format!(
        "{label}: ticks={ticks} scanned_repository_sum={scanned_sum} \
         ci_read_log_lines={ci_read_lines} last_scanned_repository_paths={last_paths}"
    )
}

fn value_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let start = line.find(key)? + key.len();
    Some(line[start..].split_whitespace().next().unwrap_or(""))
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
