//! The Forgejo twin of `tests/multiprocess.rs` (Phase 4 of
//! `plans/forgejo-e2e/README.md`).
//!
//! Same one-process-per-part rehearsal as the filesystem `multiprocess` test, but
//! against a **real Forgejo server** with a **real host-mode `forgejo-runner`**
//! producing genuine CI. For each of the four reference-delivery scenarios it:
//!
//! 1. boots a throwaway [`ForgejoServer`] + [`ForgejoRunner`] and provisions the
//!    org, repo (`auto_init`), per-role users/tokens/passwords, labels, and the
//!    CI workflow ([`provision`]);
//! 2. seeds via the scenario's **exact** seed closure against an admin
//!    [`ForgejoForge`] — the same closure the filesystem test uses;
//! 3. spawns the `harness-testing-worker` binary `--backend forgejo --clock wall`
//!    once per role-with-an-agent (each given its role's token, plus the web-UI
//!    username/password for the CI read path, via env) plus one mechanical
//!    worker, behind a kill-on-drop fleet — **no** `--kind ci` worker, because the
//!    real runner is the CI producer (read via the Phase 3b web-UI path);
//! 4. polls the scenario's **exact** assert closure (fresh forge each poll) until
//!    convergence, then stops via the `--stop-file` sentinel and asserts every
//!    child exited 0 — identical control flow to `multiprocess.rs`.
//!
//! The only per-topology difference from the filesystem test is the worker flags
//! (`--backend forgejo`, `--base-url`, `--clock wall`, `--ci-sentinel`) and the
//! secrets passed via env; the seed/assert closures are reused verbatim.
//!
//! `#[ignore]`d **and** gated behind `HARNESS_FORGEJO_E2E=1`, so the default
//! `cargo test` never downloads a binary, opens a socket, or spawns a server. Run
//! it with:
//!
//! ```sh
//! HARNESS_FORGEJO_E2E=1 \
//!   cargo test -p harness-testing --test forgejo_multiprocess -- --ignored
//! ```
//!
//! A real host CI job takes seconds and provisioning + git + HTTP add up, so the
//! convergence timeout, run-secs backstop, and poll cadence are **much** larger
//! than the filesystem test's. Each scenario boots its own server + runner for
//! full isolation (the simplest correct alternative to one-server-many-repos,
//! given provisioning targets a fixed repo).

use harness_forge::Forge;
use harness_forge_forgejo::{ForgejoConfig, ForgejoForge};
use harness_runner::{RunnerConfig, Scenario};
use harness_testing::agents::fake_registry;
use harness_testing::forgejo_server::{provision, ForgejoRunner, ForgejoServer, Provisioned};
use harness_testing::scenarios::{
    changes_requested_then_approved, ci_fails_then_passes, dependency_chain_mechanically_unblocked,
    happy_path,
};
use harness_testing::worker_bin::{FORGEJO_PASSWORD_ENV, FORGEJO_TOKEN_ENV, FORGEJO_USERNAME_ENV};
use harness_testing::{runner_config, workflow};
use harness_workflow::RoleId;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long to wait for the multi-process world to converge before failing.
///
/// A real host CI job (cold-start runner, two runs for fail→pass) takes seconds,
/// and every PR carries real git branch + HTTP round-trips, so this is far above
/// the filesystem test's 30 s.
const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(300);
/// Convergence budget for the **real-agent** topology. Each tick adds one or more
/// LLM round-trips on top of the already-slow real CI, and several roles must
/// each take an LLM-decided step in sequence (triage → claim/open PR → review →
/// align → merge → reconcile), so the ceiling is well above the fake topology's.
const REAL_AGENTS_CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(900);
/// How often the driver re-runs the assert closure while polling.
const ASSERT_POLL: Duration = Duration::from_secs(2);
/// Worker poll cadence; modest because every tick is real HTTP (findings-phase-0
/// §6 measured ~10–20 ms/call locally).
const WORKER_POLL_MS: u64 = 500;
/// Backstop run length per child, in case the driver dies before stopping it.
/// Large enough to cover the real-agent convergence budget plus teardown slack.
const WORKER_RUN_SECS: u64 = 1200;

/// The worker behavior that distinguishes one scenario's topology.
///
/// These map onto the same in-process registry wiring the filesystem test uses
/// (`--architect`/`--reviewer`), plus the Forgejo-only `--ci-sentinel` policy that
/// tells the engineer whether the PR head passes CI from the start (`present`) or
/// fails first and is fixed by a follow-up commit (`deferred`).
struct Variant {
    /// The scenario whose seed/assert closures the driver reuses.
    scenario: fn() -> Scenario,
    /// `--architect` value passed to the architect role worker.
    architect: &'static str,
    /// `--reviewer` value passed to the reviewer role worker.
    reviewer: &'static str,
    /// `--ci-sentinel` value passed to the engineer role worker.
    ci_sentinel: &'static str,
    /// `--agents` value passed to every role worker (`fake` or `real`). The
    /// `real` topology swaps the deterministic fakes for in-process LLM agents
    /// (ChatGPT OAuth by default per the cost policy) and is double-gated (see
    /// [`Agents`]).
    agents: Agents,
}

/// Which agent registry the role workers run, and how the test is gated.
///
/// The seed/assert closures and end state are **identical** across both — only
/// *who decides* changes (the whole point of reusing the scenario). `Real`
/// additionally requires `HARNESS_FORGEJO_AGENTS=1` (real LLM calls are
/// non-deterministic) and gets a larger convergence budget for LLM latency.
/// Per the cost policy the real agents default to ChatGPT OAuth (a flat
/// subscription); set `HARNESS_AGENTS_AUTH=deepseek` to opt back to DeepSeek.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Agents {
    Fake,
    Real,
}

impl Agents {
    fn flag(self) -> &'static str {
        match self {
            Agents::Fake => "fake",
            Agents::Real => "real",
        }
    }

    /// Convergence budget: real agents add LLM round-trips per tick on top of the
    /// already-slow real CI, so the real topology gets a much larger ceiling.
    fn convergence_timeout(self) -> Duration {
        match self {
            Agents::Fake => CONVERGENCE_TIMEOUT,
            Agents::Real => REAL_AGENTS_CONVERGENCE_TIMEOUT,
        }
    }
}

/// Returns whether the env opt-in for `agents` is present; prints a skip note
/// when not. The fake topology needs only `HARNESS_FORGEJO_E2E=1`; the real
/// topology additionally needs `HARNESS_FORGEJO_AGENTS=1` (real,
/// non-deterministic LLM calls), matching the B1 engineer e2e gate.
fn enabled(agents: Agents) -> bool {
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
                 `pi /login openai-codex`); set HARNESS_AGENTS_AUTH=deepseek to use a DeepSeek \
                 key via HARNESS_DEEPSEEK_API_KEY[_PATH] or .cache/deepseek-api-key)"
            );
            false
        }
    }
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; \
            run with HARNESS_FORGEJO_E2E=1 -- --ignored"]
fn happy_path_converges_against_real_forgejo() {
    run_variant(&Variant {
        scenario: happy_path,
        architect: "default",
        reviewer: "default",
        ci_sentinel: "present",
        agents: Agents::Fake,
    });
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; \
            run with HARNESS_FORGEJO_E2E=1 -- --ignored"]
fn changes_requested_then_approved_converges_against_real_forgejo() {
    run_variant(&Variant {
        scenario: changes_requested_then_approved,
        architect: "default",
        reviewer: "request-changes-then-approve",
        ci_sentinel: "present",
        agents: Agents::Fake,
    });
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; \
            run with HARNESS_FORGEJO_E2E=1 -- --ignored"]
fn ci_fails_then_passes_converges_against_real_forgejo() {
    run_variant(&Variant {
        scenario: ci_fails_then_passes,
        architect: "default",
        reviewer: "default",
        // The PR head omits `ci-ok` so the first run fails; the engineer's fix
        // commit adds it, producing a second, passing head SHA (two verdicts).
        ci_sentinel: "deferred",
        agents: Agents::Fake,
    });
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; \
            run with HARNESS_FORGEJO_E2E=1 -- --ignored"]
fn dependency_chain_mechanically_unblocked_against_real_forgejo() {
    run_variant(&Variant {
        scenario: dependency_chain_mechanically_unblocked,
        architect: "closing",
        reviewer: "default",
        ci_sentinel: "present",
        agents: Agents::Fake,
    });
}

// ---------------------------------------------------------------------------
// Real-agent topology (Phase B2). Same scenarios, same seed/assert closures, but
// every role worker runs an in-process LLM agent (ChatGPT OAuth by default; `--agents
// real`) instead of the deterministic fakes. Double-gated behind
// HARNESS_FORGEJO_E2E=1 **and** HARNESS_FORGEJO_AGENTS=1.
//
// The **happy path** is B2's bar (it must converge). The harder scenarios are
// attempted too; if a real run is flaky, see `findings-phase-b.md` for the
// known-limitation note rather than forcing it.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "boots a real Forgejo + runner and makes real LLM calls; \
            run with HARNESS_FORGEJO_E2E=1 HARNESS_FORGEJO_AGENTS=1 -- --ignored"]
fn happy_path_converges_with_real_agents() {
    run_variant(&Variant {
        scenario: happy_path,
        architect: "default",
        reviewer: "default",
        ci_sentinel: "present",
        agents: Agents::Real,
    });
}

#[test]
#[ignore = "boots a real Forgejo + runner and makes real LLM calls; \
            run with HARNESS_FORGEJO_E2E=1 HARNESS_FORGEJO_AGENTS=1 -- --ignored"]
fn changes_requested_then_approved_with_real_agents() {
    run_variant(&Variant {
        scenario: changes_requested_then_approved,
        architect: "default",
        reviewer: "request-changes-then-approve",
        ci_sentinel: "present",
        agents: Agents::Real,
    });
}

#[test]
#[ignore = "boots a real Forgejo + runner and makes real LLM calls; \
            run with HARNESS_FORGEJO_E2E=1 HARNESS_FORGEJO_AGENTS=1 -- --ignored"]
fn ci_fails_then_passes_with_real_agents() {
    run_variant(&Variant {
        scenario: ci_fails_then_passes,
        architect: "default",
        reviewer: "default",
        ci_sentinel: "deferred",
        agents: Agents::Real,
    });
}

#[test]
#[ignore = "boots a real Forgejo + runner and makes real LLM calls; \
            run with HARNESS_FORGEJO_E2E=1 HARNESS_FORGEJO_AGENTS=1 -- --ignored"]
fn dependency_chain_mechanically_unblocked_with_real_agents() {
    run_variant(&Variant {
        scenario: dependency_chain_mechanically_unblocked,
        architect: "closing",
        reviewer: "default",
        ci_sentinel: "present",
        agents: Agents::Real,
    });
}

/// Drives one scenario through the true multi-process topology against a real
/// Forgejo, asserting it converges to the same end state as the in-process
/// scenario.
fn run_variant(variant: &Variant) {
    if !enabled(variant.agents) {
        return;
    }

    // For the real-agent topology, fail fast with a clear message if the
    // credential is missing — before booting a server or making any LLM call. The
    // error never includes secret bytes. Defaults to ChatGPT OAuth (the cost
    // policy: a flat subscription, not pay-per-token DeepSeek); opt back to
    // DeepSeek with HARNESS_AGENTS_AUTH=deepseek.
    if variant.agents == Agents::Real {
        harness_agents::ProviderConfig::from_auth(agents_auth_choice(), None, None).expect(
            "LLM provider config builds for --agents real \
             (ChatGPT OAuth: run `pi /login openai-codex`; \
             DeepSeek: set HARNESS_DEEPSEEK_API_KEY[_PATH] or place .cache/deepseek-api-key)",
        );
    }

    // Boot the server + runner. `ForgejoServer::start` polls readiness with a
    // *blocking* reqwest client whose nested runtime must live and die off any
    // async reactor — this driver is a plain `#[test]`, so there is no reactor to
    // collide with, and the worker processes carry their own Tokio runtimes.
    let server = ForgejoServer::start().expect("forgejo server boots");
    let mut runner = ForgejoRunner::register(&server).expect("forgejo runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");

    let provisioned = block_on_provision(&server);
    let config = runner_config();

    // Seed via the scenario's exact seed closure against an admin-owner backend.
    let scenario = (variant.scenario)();
    seed(&server, &provisioned, &scenario);

    let stop_file = server_stop_file(&provisioned);
    let _ = std::fs::remove_file(&stop_file);

    // Spawn one process per moving part behind a kill-on-drop guard.
    let mut workers = WorkerFleet::spawn(&server, &provisioned, &stop_file, &config, variant);

    // Detect convergence in-process by polling the exact scenario assert.
    let timeout = variant.agents.convergence_timeout();
    let converged = poll_until_converged(&server, &provisioned, &scenario, timeout);

    // Stop: touch the sentinel and join every child.
    touch(&stop_file);
    let exits = workers.wait_all();

    if let Err(error) = converged {
        let diag = ci_diagnostics(&server, &provisioned);
        panic!(
            "multi-process world did not converge within {timeout:?}: {error}\n\
             runner running={}, runner log tail:\n{}\n--- CI diagnostics ---\n{}",
            runner.is_running(),
            runner.log_tail(),
            diag
        );
    }

    for (label, status) in &exits {
        assert!(
            status.success(),
            "worker '{label}' exited with {status:?}, expected success"
        );
    }

    // Run the assert once more for a clean message.
    let forge = admin_forge(&server, &provisioned);
    let assert = (scenario.assert)(&forge, &provisioned.repository);
    futures_block_on(assert).expect("final scenario assertion passes after workers stop");

    let _ = std::fs::remove_file(&stop_file);
    drop(workers);
    drop(runner);
    drop(server);
}

/// Runs the async [`provision`] to completion on a one-shot current-thread Tokio
/// runtime (the driver is a sync `#[test]`).
fn block_on_provision(server: &ForgejoServer) -> Provisioned {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds");
    runtime
        .block_on(provision(server))
        .expect("provisioning succeeds")
}

/// Builds an admin-owner [`ForgejoForge`] (admin token, default repo + web-UI
/// credentials) for seeding and asserting against the provisioned repo.
fn admin_forge(server: &ForgejoServer, provisioned: &Provisioned) -> ForgejoForge {
    // The admin can read CI through the web UI with the shared role password; the
    // admin login is not a role user, so reuse a provisioned role's web-UI creds
    // for the CI read fallback during asserts.
    let config = ForgejoConfig::new(server.base_url(), &provisioned.admin_token)
        .with_default_repo(&provisioned.owner, &provisioned.name);
    let config = match any_role(provisioned) {
        Some(role) => config.with_web_ui_credentials(&role.user, &role.password),
        None => config,
    };
    ForgejoForge::new(config)
}

/// Any provisioned role identity, used only for its web-UI credentials during
/// admin-side CI reads.
fn any_role(provisioned: &Provisioned) -> Option<&harness_testing::forgejo_server::RoleIdentity> {
    runner_config()
        .role_bindings
        .iter()
        .find_map(|binding| provisioned.role(&binding.role))
}

/// Lists each PR, its head branch, and its CI jobs for a convergence-failure
/// message. Best-effort: any read error is folded into the string.
fn ci_diagnostics(server: &ForgejoServer, provisioned: &Provisioned) -> String {
    use harness_forge::{CiJobQuery, PullRequestQuery};
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
                    match forge.list_pull_request_reviews(&pr.id).await {
                        Ok(reviews) => {
                            for r in reviews {
                                out.push_str(&format!(
                                    "  review by {} decision={:?} at={}\n",
                                    r.reviewer_id.as_str(),
                                    r.decision,
                                    r.submitted_at
                                ));
                            }
                        }
                        Err(error) => out.push_str(&format!("  list reviews error: {error}\n")),
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

/// Seeds the provisioned repo in-process using the scenario's exact seed closure.
fn seed(server: &ForgejoServer, provisioned: &Provisioned, scenario: &Scenario) {
    let forge = admin_forge(server, provisioned);
    let future = (scenario.seed)(&forge, &provisioned.repository);
    futures_block_on(future).expect("seeding the scenario succeeds");
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

fn server_stop_file(provisioned: &Provisioned) -> PathBuf {
    let id = NEXT_STOP.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "harness-forgejo-mp-stop-{}-{}-{id}",
        provisioned.name,
        std::process::id()
    ))
}

static NEXT_STOP: AtomicU64 = AtomicU64::new(0);

fn touch(path: &Path) {
    std::fs::write(path, b"stop").expect("writing the stop sentinel succeeds");
}

/// Env var the worker reads the DeepSeek API key from (direct value).
const DEEPSEEK_API_KEY_ENV: &str = "HARNESS_DEEPSEEK_API_KEY";

/// The agent auth mode for the real-agent topology: ChatGPT OAuth by default
/// (the cost policy — a flat subscription, not pay-per-token DeepSeek),
/// overridable to DeepSeek with `HARNESS_AGENTS_AUTH=deepseek`. The spawned
/// workers default to the same mode (their `--auth` flag defaults to
/// chatgpt-oauth), but the driver passes `--auth` explicitly so a deepseek
/// override also reaches the children.
fn agents_auth_choice() -> harness_agents::AuthChoice {
    match std::env::var("HARNESS_AGENTS_AUTH").ok().as_deref() {
        Some("deepseek") => harness_agents::AuthChoice::DeepSeek,
        _ => harness_agents::AuthChoice::ChatGptOAuth,
    }
}

/// The `--auth` flag value matching [`agents_auth_choice`].
fn agents_auth_flag() -> &'static str {
    match agents_auth_choice() {
        harness_agents::AuthChoice::DeepSeek => "deepseek",
        harness_agents::AuthChoice::ChatGptOAuth => "chatgpt-oauth",
    }
}

/// Resolves the DeepSeek API key the same way `harness_agents::ProviderConfig`
/// does, so it can be passed explicitly to each real-agent worker child (whose
/// CWD differs from the workspace root and so cannot resolve the default relative
/// `.cache/deepseek-api-key`). Resolution order: `HARNESS_DEEPSEEK_API_KEY`
/// (direct), else the file at `HARNESS_DEEPSEEK_API_KEY_PATH`, else the
/// workspace-root `.cache/deepseek-api-key`. The key is never logged; the driver
/// already validated it builds via `ProviderConfig::from_auth`. Only used for the
/// DeepSeek opt-out; the ChatGPT OAuth path reads the absolute shared auth file.
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
            // CARGO_MANIFEST_DIR is crates/harness-testing; the key lives at the
            // workspace root's .cache/.
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

/// A spawned worker process labelled for clear failure messages.
struct SpawnedWorker {
    label: String,
    child: Child,
}

/// Owns every spawned worker process and kills any survivors on drop.
struct WorkerFleet {
    workers: Vec<SpawnedWorker>,
}

impl WorkerFleet {
    /// Spawns one `--backend forgejo` process per role-with-an-agent, plus the
    /// mechanical worker. No CI worker — the real runner is the CI producer.
    fn spawn(
        server: &ForgejoServer,
        provisioned: &Provisioned,
        stop_file: &Path,
        config: &RunnerConfig,
        variant: &Variant,
    ) -> Self {
        let base = server.base_url().to_string();
        let repo = format!("{}/{}", provisioned.owner, provisioned.name);
        let mut workers = Vec::new();

        // For `--agents real` with the DeepSeek opt-out, each role worker reads
        // the DeepSeek key from the env. Resolve it once in the driver and pass it
        // explicitly (the child's CWD is the crate dir, so the default relative
        // `.cache/...` would not resolve). The default ChatGPT OAuth path needs no
        // key — it reads the absolute shared `~/.pi/agent/auth.json`. Passed via
        // env, never argv; empty for `--agents fake`.
        let auth_flag = agents_auth_flag();
        let deepseek_key = match (variant.agents, agents_auth_choice()) {
            (Agents::Real, harness_agents::AuthChoice::DeepSeek) => Some(resolve_deepseek_key()),
            _ => None,
        };

        // Derive role workers from the compiled workflow ∩ registered fake agents
        // ∩ configured role bindings — never a hardcoded list.
        for role in role_workers(config) {
            let identity = provisioned
                .role(&RoleId::new(&role))
                .unwrap_or_else(|| panic!("role '{role}' is provisioned with an identity"));
            // Each role acts as its own token; every role is given web-UI
            // credentials too so whichever role observes a CI gate can read it via
            // the Phase 3b fallback. Real-agent roles additionally get the
            // DeepSeek key.
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
                &repo,
                stop_file,
                &[
                    ("--kind", "role"),
                    ("--role", &role),
                    ("--user", &identity.user),
                    ("--architect", variant.architect),
                    ("--reviewer", variant.reviewer),
                    ("--ci-sentinel", variant.ci_sentinel),
                    ("--agents", variant.agents.flag()),
                    ("--auth", auth_flag),
                ],
                &env,
            );
            workers.push(SpawnedWorker {
                label: format!("role:{role}"),
                child,
            });
        }

        // The mechanical worker needs a write token but no web-UI creds; use the
        // admin token so it can reconcile regardless of role.
        let child = spawn_worker(
            &base,
            &repo,
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

/// Role ids that have both a registered fake agent and a configured binding —
/// the same derivation the filesystem multiprocess test uses, so the worker set
/// is never hardcoded.
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
    repo: &str,
    stop_file: &Path,
    extra: &[(&str, &str)],
    env: &[(&str, &str)],
) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_harness-testing-worker"));
    command
        .arg("--backend")
        .arg("forgejo")
        .arg("--base-url")
        .arg(base_url)
        .arg("--repo")
        .arg(repo)
        // `--root` is required by the parser but unused by the Forgejo backend.
        .arg("--root")
        .arg(std::env::temp_dir().join("harness-forgejo-mp-unused"))
        .arg("--clock")
        .arg("wall")
        .arg("--poll-ms")
        .arg(WORKER_POLL_MS.to_string())
        .arg("--stop-file")
        .arg(stop_file)
        .arg("--run-secs")
        .arg(WORKER_RUN_SECS.to_string());
    for (flag, value) in extra {
        command.arg(flag).arg(value);
    }
    // Secrets travel via env, never argv. Clear inherited Forgejo vars first so a
    // stray value from the operator's shell cannot leak into a child.
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
