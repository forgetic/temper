//! `temper run` live e2e: one in-process daemon + worker + agent against real
//! Forgejo and a fake LLM.
//!
//! This is the single-process analog of `daemon_forgejo_e2e`: instead of
//! spawning a separate daemon binary and a deterministic wire-protocol worker,
//! it spawns **one `temper run`** process that hosts the daemon, the worker, and
//! the coding agent on one event loop. The agent's LLM traffic is redirected to
//! a local jig `FakeLlm` (no real credentials), and the forge is the shared
//! throwaway Forgejo fixture from `../bench`. The fake engineer turn writes a
//! product file; the test asserts an engineer-authored implementation PR is
//! opened for the seeded issue with that file in its diff.
//!
//! Stops at **PR opened** (not merged): merging is gated on real Actions CI,
//! which `daemon_forgejo_e2e` already covers. This test's job is the new
//! single-process path — agent → diff → PR — end to end.
//!
//! Sets `ANVIL_TEST_PROVIDER_BASE_URL` so `temper run` points the agent at the
//! fake LLM. Run with:
//!   cargo test --test run_forgejo_e2e -- --ignored

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_forge::{ItemNumber, PullRequest, PullRequestQuery, UserId};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_testing::forgejo_runtime::RunWorkspace;
use temper_testing::forgejo_server::{
    ForgejoServer, Provisioned, start_cached_provisioned_repositories,
};

const ENGINEER: &str = "engineer";
const REPO_NAME: &str = "temper-run-e2e";
const DEFAULT_CONVERGENCE_SECS: u64 = 180;

/// Convergence budget, overridable via `TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS`
/// (the same knob the daemon e2e honors). CI sets it higher because a cold
/// machine spends ~2min provisioning the Forgejo fixture before any work runs.
fn convergence_timeout() -> Duration {
    std::env::var("TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_CONVERGENCE_SECS))
}
// A delivery workflow with NO ci_gate on landing: the engineer agent opens a PR
// and there is no Actions dependency, so the test converges on PR-open without a
// runner. (Reuses the shapes of the canonical basic-delivery workflow.)
const RUN_WORKFLOW: &str = include_str!("run-delivery.json");

#[test]
#[ignore = "boots a real Forgejo fixture + a fake LLM and spawns `temper run`; run with --ignored"]
fn temper_run_opens_an_engineer_pr_via_fake_llm() {
    let started = Instant::now();

    // --- World: provisioned Forgejo (org, engineer identity, labels, repo) ---
    let cached = start_cached_provisioned_repositories(&[REPO_NAME.to_string()])
        .expect("forgejo provisioned world starts");
    let server = cached.server;
    let provisioned = cached
        .state
        .provisioned(REPO_NAME)
        .unwrap_or_else(|| panic!("provisioned world has no repo named {REPO_NAME}"));
    let engineer = provisioned
        .role(&temper_workflow::RoleId::new(ENGINEER))
        .expect("engineer identity is provisioned")
        .clone();
    eprintln!(
        "run_forgejo_e2e world up: cache_hit={} startup={:?}",
        cached.cache_hit,
        started.elapsed()
    );

    // --- Fake LLM: one engineer turn writes the product file, then summarizes ---
    let observed_continuation = Arc::new(AtomicUsize::new(0));
    let fake = engineer_fake(Arc::clone(&observed_continuation));

    let workspace = RunWorkspaceGuard::new("temper-run-forgejo-e2e");
    let workflow_file = workspace.0.write_file("run/workflow.json", RUN_WORKFLOW);
    let run_log = workspace.0.join("run/temper-run.log");

    // --- Spawn one `temper run` (daemon + worker + agent in one process) ---
    let mut run = spawn_temper_run(
        &server,
        &provisioned,
        &engineer,
        &workflow_file,
        workspace.0.path(),
        &fake.base_url(),
        &run_log,
    );

    // --- Seed one raw intake issue; the mechanical backstop stamps it
    //     code+ready, then the engineer agent opens a PR. ---
    let issue = block_on(temper_testing::forgejo_server::seed_intake_issue(
        server.base_url(),
        &provisioned.admin_token,
        &provisioned.owner,
        &provisioned.name,
    ))
    .expect("seed intake issue");
    eprintln!("run_forgejo_e2e seeded intake issue #{issue}");

    // --- Converge: an engineer-authored implementation PR for this issue ---
    let forge = admin_forge(&server, &provisioned);
    let timeout = convergence_timeout();
    let deadline = Instant::now() + timeout;
    let result = loop {
        if let Some(status) = run.child.try_wait().expect("run try_wait") {
            panic!(
                "`temper run` exited early with {status:?}\n--- temper run log ---\n{}",
                run.log_tail()
            );
        }
        match block_on(find_engineer_pr(&forge, &provisioned, issue, &engineer)) {
            Ok(pr) => break pr,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(error) => panic!(
                "`temper run` did not open an engineer PR within {timeout:?}: {error}\n--- temper run log ---\n{}",
                run.log_tail()
            ),
        }
    };

    eprintln!(
        "run_forgejo_e2e converged: PR #{} authored by {:?} in {:?}",
        result.number,
        result.author_id,
        started.elapsed()
    );
    assert!(
        observed_continuation.load(Ordering::SeqCst) >= 1,
        "fake LLM never saw a tool-result continuation — the agent did not run its loop"
    );

    // Graceful-ish shutdown: SIGTERM the run process; the guard hard-kills on drop.
    let _ = run.child.kill();
}

/// The fake engineer agent: first turn writes the product file via the `write`
/// tool, second turn returns the result JSON. Mirrors the
/// `temper-agent` jig coding-agent fixture.
fn engineer_fake(observed_continuation: Arc<AtomicUsize>) -> FakeLlm {
    // The agent's cwd is the workspace root; the single repo is checked out in
    // its sibling dir (the repo's last path segment, ADR 0023). Write the product
    // into that subdir so the worker sees a diff in the repo and pushes it.
    let product_path = format!("{REPO_NAME}/DELIVERY.md");
    FakeLlm::start(Script::rule(move |view| {
        if view.prior_tool_results == 0 {
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_write".to_string(),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": product_path.clone(),
                        "content": "delivered by temper run\n"
                    }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        } else {
            observed_continuation.fetch_add(1, Ordering::SeqCst);
            Reply::text(r#"{"summary":"Created DELIVERY.md."}"#)
        }
    }))
    .expect("start fake LLM")
}

/// Find the engineer-authored implementation PR correlated to `issue`.
async fn find_engineer_pr(
    forge: &ForgejoForge,
    provisioned: &Provisioned,
    issue: ItemNumber,
    engineer: &temper_testing::forgejo_server::RoleIdentity,
) -> Result<PullRequest, String> {
    let prs: Vec<PullRequest> = forge
        .list_pull_requests(&provisioned.repository, PullRequestQuery::default())
        .await
        .map_err(|error| format!("list_pull_requests failed: {error}"))?
        .into_iter()
        .filter(|pr| pr.labels.iter().any(|label| label == "implementation"))
        .collect();
    let pr = match prs.len() {
        0 => return Err("no implementation PR yet".to_string()),
        1 => prs.into_iter().next().expect("one PR"),
        n => return Err(format!("expected one implementation PR, found {n}")),
    };
    let metadata = temper_workflow::parse_metadata_block(&pr.body)
        .map_err(|error| format!("PR metadata malformed: {error}"))?
        .ok_or("PR missing workflow metadata")?;
    let expected = format!("pr-for-code-{issue}");
    if metadata.correlation_key.as_deref() != Some(expected.as_str()) {
        return Err(format!(
            "PR correlation key {:?} != {expected:?}",
            metadata.correlation_key
        ));
    }
    if pr.author_id != UserId::new(engineer.user.clone()) {
        return Err(format!(
            "PR #{} authored by {:?}, not the engineer identity {:?}",
            pr.number, pr.author_id, engineer.user
        ));
    }
    Ok(pr)
}

#[allow(clippy::too_many_arguments)]
fn spawn_temper_run(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    engineer: &temper_testing::forgejo_server::RoleIdentity,
    workflow_file: &Path,
    workspace: &Path,
    fake_llm_url: &str,
    log: &Path,
) -> ChildGuard {
    // The new CLI is config-file driven: standalone `temper daemon` (no
    // --service) runs engine + worker + agent in one process. Deployment
    // settings go in the config file; secrets (forge token, the engineer's
    // role identity, the dummy LLM key) and the fake-LLM redirect come from the
    // environment, which the resolver layers over the file.
    let config_path = workspace.join("run-config.toml");
    let config = format!(
        "schema_version = 1\n\
         [forge]\n\
         type = \"forgejo\"\n\
         url = \"{base_url}\"\n\
         [engine]\n\
         repos = [\"{owner}/{name}\"]\n\
         roles = [\"{ENGINEER}\"]\n\
         workflow = \"{workflow}\"\n\
         poll_cadence_secs = 2\n\
         mechanical_cadence_secs = 2\n\
         daemon_id = \"temper-run-e2e\"\n\
         [worker]\n\
         worker_id = \"temper-run-e2e-worker\"\n\
         workspace = \"{workspace_root}\"\n\
         git_base_url = \"{base_url}\"\n\
         [agent]\n\
         provider = \"deepseek\"\n\
         max_iterations = 6\n",
        base_url = server.base_url(),
        owner = provisioned.owner,
        name = provisioned.name,
        workflow = workflow_file.display(),
        workspace_root = workspace.join("run/agent-workspaces").display(),
    );
    std::fs::write(&config_path, config).expect("write run config");

    let log_file = log_file(log);
    let engineer_upper = ENGINEER.to_uppercase();
    let child = Command::new(env!("CARGO_BIN_EXE_temper"))
        .arg("daemon")
        .arg("--config")
        .arg(&config_path)
        // Forge connection (token is secret -> env).
        .env("FORGEJO_ACCESS_TOKEN", &provisioned.admin_token)
        .env("FORGEJO_USERNAME", &engineer.user)
        .env("FORGEJO_PASSWORD", &engineer.password)
        // Per-role Forge API token routing.
        .env(
            format!("TEMPER_FORGEJO_TOKEN_{engineer_upper}"),
            &engineer.token,
        )
        // Per-role git identity the agent pushes with.
        .env(
            format!("TEMPER_FORGEJO_USER_{engineer_upper}"),
            &engineer.user,
        )
        .env(
            format!("TEMPER_FORGEJO_EMAIL_{engineer_upper}"),
            &engineer.email,
        )
        // Agent LLM: dummy DeepSeek key (preflight is a no-op for ApiKey mode)
        // redirected to the local fake LLM.
        .env("TEMPER_DEEPSEEK_API_KEY", "sk-jig-test")
        .env("ANVIL_TEST_PROVIDER_BASE_URL", fake_llm_url)
        .env_remove("FORGEJO_URL")
        .env_remove("FORGEJO_DEFAULT_REPO")
        .stdout(Stdio::from(log_file.try_clone().expect("log clones")))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("standalone `temper daemon` binary spawns");
    ChildGuard {
        label: "temper daemon",
        child,
        log: log.to_path_buf(),
    }
}

fn admin_forge(server: &ForgejoServer, provisioned: &Provisioned) -> ForgejoForge {
    ForgejoForge::new(ForgejoConfig::new(
        server.base_url().to_string(),
        provisioned.admin_token.clone(),
    ))
}

/// Borrow-friendly block_on (no `'static` bound), matching the daemon e2e
/// harness: builds a throwaway engine runtime and drives one future on it.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    temper_engine_io::build_runtime()
        .expect("engine runtime builds")
        .block_on(future)
}

// --- small process/log/workspace guards (self-contained) ---

struct RunWorkspaceGuard(RunWorkspace);

impl RunWorkspaceGuard {
    fn new(prefix: &str) -> Self {
        Self(RunWorkspace::new(prefix))
    }
}

struct ChildGuard {
    label: &'static str,
    child: Child,
    log: PathBuf,
}

impl ChildGuard {
    fn log_tail(&self) -> String {
        // Copy the full log to a stable path for post-mortem, then return the
        // interesting lines (drop the high-volume mechanical/reconciliation
        // summaries so the worker/agent lifecycle is visible).
        let contents = std::fs::read_to_string(&self.log).unwrap_or_default();
        let preserved = std::env::temp_dir().join("temper-run-e2e-last.log");
        let _ = std::fs::write(&preserved, &contents);
        let interesting: Vec<&str> = contents
            .lines()
            .filter(|line| {
                !line.contains("mechanical_automation_summary")
                    && !line.contains("mechanical_reconciliation_summary")
            })
            .collect();
        let tail = interesting.len().saturating_sub(80);
        format!(
            "(full log: {})\n{}",
            preserved.display(),
            interesting[tail..].join("\n")
        )
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        eprintln!("[{}] killed on drop", self.label);
    }
}

fn log_file(path: &Path) -> std::fs::File {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open log file")
}
