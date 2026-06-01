use harness_forge::Forge;
use harness_runner::RunnerConfig;
use harness_testing::agents::fake_registry;
use harness_testing::forgejo_server::{ForgejoServer, Provisioned};
use harness_testing::worker_bin::{FORGEJO_PASSWORD_ENV, FORGEJO_TOKEN_ENV, FORGEJO_USERNAME_ENV};
use harness_testing::workflow;
use harness_workflow::RoleId;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(300);
const REAL_AGENTS_CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(900);
const DEEPSEEK_API_KEY_ENV: &str = "HARNESS_DEEPSEEK_API_KEY";

/// Which agent registry the role workers run, and how the test is gated.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Agents {
    Fake,
    Real,
}

impl Agents {
    pub fn flag(self) -> &'static str {
        match self {
            Agents::Fake => "fake",
            Agents::Real => "real",
        }
    }

    /// Convergence budget: real agents add LLM round-trips per tick on top of the
    /// already-slow real CI, so the real topology gets a much larger ceiling.
    pub fn convergence_timeout(self) -> Duration {
        match self {
            Agents::Fake => CONVERGENCE_TIMEOUT,
            Agents::Real => REAL_AGENTS_CONVERGENCE_TIMEOUT,
        }
    }
}

/// Returns whether the env opt-in for `agents` is present; prints a skip note
/// when not. The fake topology needs only `HARNESS_FORGEJO_E2E=1`; the real
/// topology additionally needs `HARNESS_FORGEJO_AGENTS=1`.
pub fn enabled(agents: Agents) -> bool {
    let e2e = std::env::var("HARNESS_FORGEJO_E2E").ok().as_deref() == Some("1");
    match agents {
        Agents::Fake => {
            if e2e {
                return true;
            }
            eprintln!(
                "skipping Forgejo multiprocess e2e test: set HARNESS_FORGEJO_E2E=1 to enable \
                 (downloads pinned Forgejo + forgejo-runner binaries and spawns a host-mode runner)"
            );
            false
        }
        Agents::Real => {
            let real = std::env::var("HARNESS_FORGEJO_AGENTS").ok().as_deref() == Some("1");
            if e2e && real {
                return true;
            }
            eprintln!(
                "skipping Forgejo real-agent multiprocess e2e: set BOTH HARNESS_FORGEJO_E2E=1 and \
                 HARNESS_FORGEJO_AGENTS=1 (boots a real Forgejo + runner and makes real, \
                 non-deterministic LLM calls). Defaults to ChatGPT OAuth (run \
                 `pi /login openai-codex`); set HARNESS_AGENTS_AUTH=anthropic-oauth for \
                 Anthropic OAuth (`pi /login anthropic`) or HARNESS_AGENTS_AUTH=deepseek \
                 to use a DeepSeek key via HARNESS_DEEPSEEK_API_KEY[_PATH] or \
                 .cache/deepseek-api-key)"
            );
            false
        }
    }
}

/// The agent auth mode for the real-agent topology: ChatGPT OAuth by default
/// (the cost policy — a flat subscription, not pay-per-token DeepSeek),
/// overridable to DeepSeek with `HARNESS_AGENTS_AUTH=deepseek`.
pub fn agents_auth_choice() -> harness_agents::AuthChoice {
    match std::env::var("HARNESS_AGENTS_AUTH").ok().as_deref() {
        Some("deepseek") => harness_agents::AuthChoice::DeepSeek,
        Some("anthropic-oauth") => harness_agents::AuthChoice::AnthropicOAuth,
        _ => harness_agents::AuthChoice::ChatGptOAuth,
    }
}

fn agents_auth_flag() -> &'static str {
    match agents_auth_choice() {
        harness_agents::AuthChoice::DeepSeek => "deepseek",
        harness_agents::AuthChoice::ChatGptOAuth => "chatgpt-oauth",
        harness_agents::AuthChoice::AnthropicOAuth => "anthropic-oauth",
    }
}

/// Resolves the DeepSeek API key the same way `harness_agents::ProviderConfig`
/// does, so it can be passed explicitly to each real-agent worker child.
fn resolve_deepseek_key() -> String {
    if let Ok(key) = std::env::var(DEEPSEEK_API_KEY_ENV) {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let path = std::env::var("HARNESS_DEEPSEEK_API_KEY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.cache/deepseek-api-key")
        });
    std::fs::read_to_string(&path)
        .map(|raw| raw.trim().to_string())
        .unwrap_or_else(|error| {
            panic!(
                "could not read DeepSeek API key for --agents real from {}: {error}",
                path.display()
            )
        })
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
        agents: Agents,
    ) -> Self {
        let base = server.base_url().to_string();
        let mut workers = Vec::new();
        let auth_flag = agents_auth_flag();
        let deepseek_key = match (agents, agents_auth_choice()) {
            (Agents::Real, harness_agents::AuthChoice::DeepSeek) => Some(resolve_deepseek_key()),
            _ => None,
        };

        for role in role_workers(config) {
            let identity = provisioned
                .role(&RoleId::new(&role))
                .unwrap_or_else(|| panic!("role '{role}' is provisioned with an identity"));
            let mut env: Vec<(&str, &str)> = vec![
                (FORGEJO_TOKEN_ENV, identity.token.as_str()),
                (FORGEJO_USERNAME_ENV, identity.user.as_str()),
                (FORGEJO_PASSWORD_ENV, identity.password.as_str()),
            ];
            if let Some(key) = &deepseek_key {
                env.push((DEEPSEEK_API_KEY_ENV, key.as_str()));
            }
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
                    ("--agents", agents.flag()),
                    ("--auth", auth_flag),
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_harness-testing-worker"));
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
        .arg(std::env::temp_dir().join("harness-forgejo-mp-unused"))
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
