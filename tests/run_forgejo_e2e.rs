//! `temper run` live e2e: one in-process daemon + worker + agent against real
//! Forgejo and a fake LLM.
//!
//! This is the single-process analog of `daemon_forgejo_e2e`: instead of
//! spawning a separate daemon binary and a deterministic wire-protocol worker,
//! it spawns **one `temper run`** process that hosts the daemon, the worker, and
//! the coding agent on one event loop. The agent's LLM traffic is redirected to
//! a local jig `FakeLlm` (no real credentials), and the forge is the shared
//! throwaway Forgejo fixture from `../bench`. The fake engineer writes a
//! product file, calls `checkpoint` for the completed milestone, and returns a
//! final summary. The test asserts that the implementation PR opens only from
//! the real product diff and contains no model-authored plan checklist.
//!
//! Stops at **PR opened/finalized** (not merged): merging is gated on real
//! Actions CI, which `daemon_forgejo_e2e` already covers. This test's job is the
//! single-process path — agent checkpoint → final PR handoff — end to end.
//!
//! The fake-LLM base URL and a dummy DeepSeek api-key credential are supplied
//! through the config/credentials files (the agent reads no provider env). Run
//! with:
//!   cargo test --test run_forgejo_e2e -- --ignored

#![cfg(unix)]

#[path = "support/e2e_lock.rs"]
mod e2e_lock;

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
    ForgejoRunner, ForgejoServer, Provisioned, start_cached_provisioned_repositories,
};

const ENGINEER: &str = "engineer";
const REPO_NAME: &str = "temper-run-e2e";
const DEFAULT_CONVERGENCE_SECS: u64 = 300;

/// Convergence budget, overridable via `TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS`
/// (the same knob the daemon e2e honors). CI sets it explicitly; the default
/// also keeps a full five-minute window for local `--ignored` runs because the
/// checkpoint-opened PR can briefly contend with the host runner before final
/// summary handoff.
fn convergence_timeout() -> Duration {
    std::env::var("TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_CONVERGENCE_SECS))
}
// A delivery workflow tuned for this test: the engineer opens and finalizes an
// implementation PR from a real product diff while a real host-mode runner is
// alive. The test converges on PR-open/finalization; the daemon/runner CI-merge
// path is covered by `daemon_forgejo_e2e`.
const RUN_WORKFLOW: &str = include_str!("run-delivery.json");

#[test]
#[ignore = "boots a real Forgejo fixture + a fake LLM and spawns `temper run`; run with --ignored"]
fn temper_run_opens_pr_from_checkpointed_product_diff_via_fake_llm() {
    let _e2e_lock = e2e_lock::acquire();
    let started = Instant::now();

    // --- World: provisioned Forgejo (org, engineer identity, labels, repo) ---
    let cached = start_cached_provisioned_repositories(&[REPO_NAME.to_string()])
        .expect("forgejo provisioned world starts");
    let server = cached.server;
    let mut runner = ForgejoRunner::register(&server).expect("forgejo runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");
    let provisioned = cached
        .state
        .provisioned(REPO_NAME)
        .unwrap_or_else(|| panic!("provisioned world has no repo named {REPO_NAME}"));
    let engineer = provisioned
        .role(&temper_workflow::RoleId::new(ENGINEER))
        .expect("engineer identity is provisioned")
        .clone();
    eprintln!(
        "run_forgejo_e2e world up: cache_hit={} runner={} startup={:?}",
        cached.cache_hit,
        runner.is_running(),
        started.elapsed()
    );

    // --- Fake LLM: write the product file, checkpoint it, then return final
    //     success. ---
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

    // --- Converge: the engineer-authored implementation PR is finalized after
    //     the checkpointed product diff, with no implementation-plan checklist. ---
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
        match block_on(find_final_engineer_pr(
            &forge,
            &provisioned,
            issue,
            &engineer,
        )) {
            Ok(pr) => break pr,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(error) => panic!(
                "`temper run` did not finalize the engineer PR within {timeout:?}: {error}\n--- temper run log ---\n{}",
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

/// The fake engineer agent writes the product file, checkpoints the completed
/// milestone, and returns summary-only success JSON.
fn engineer_fake(observed_continuation: Arc<AtomicUsize>) -> FakeLlm {
    // The agent's cwd is the workspace root; the single repo is checked out in
    // its sibling dir (the repo's last path segment, ADR 0023). Write the product
    // into that subdir so the worker sees a diff in the repo and pushes it.
    let product_path = format!("{REPO_NAME}/DELIVERY.md");
    let turn_index = Arc::new(AtomicUsize::new(0));
    FakeLlm::start(Script::rule(move |_view| {
        let turn = turn_index.fetch_add(1, Ordering::SeqCst);
        match turn {
            0 => Reply {
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
            },
            1 => Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_checkpoint".to_string(),
                    name: "checkpoint".to_string(),
                    args: serde_json::json!({ "label": "Create delivery file" }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            },
            _ => {
                observed_continuation.fetch_add(1, Ordering::SeqCst);
                Reply::text(r#"{"summary":"Created DELIVERY.md via checkpoint-only flow."}"#)
            }
        }
    }))
    .expect("start fake LLM")
}

/// Find the finalized engineer-authored implementation PR correlated to `issue`.
async fn find_final_engineer_pr(
    forge: &ForgejoForge,
    provisioned: &Provisioned,
    issue: ItemNumber,
    engineer: &temper_testing::forgejo_server::RoleIdentity,
) -> Result<PullRequest, String> {
    let pr = only_implementation_pr(forge, &provisioned.repository).await?;
    verify_correlated_engineer_pr(&pr, issue, &engineer.user)?;
    if !pr
        .body
        .contains("Summary: Created DELIVERY.md via checkpoint-only flow.")
    {
        return Err("final PR missing final success summary".to_string());
    }
    if pr.body.contains("Implementation plan") || pr.body.contains("- [ ]") {
        return Err(format!(
            "final PR unexpectedly contained a model-authored checklist:\n{}",
            pr.body
        ));
    }
    Ok(pr)
}

async fn only_implementation_pr(
    forge: &ForgejoForge,
    repository: &temper_forge::RepositoryId,
) -> Result<PullRequest, String> {
    let prs: Vec<PullRequest> = forge
        .list_pull_requests(repository, PullRequestQuery::default())
        .await
        .map_err(|error| format!("list_pull_requests failed: {error}"))?
        .into_iter()
        .filter(|pr| pr.labels.iter().any(|label| label == "implementation"))
        .collect();
    match prs.len() {
        0 => Err("no implementation PR yet".to_string()),
        1 => Ok(prs.into_iter().next().expect("one PR")),
        n => Err(format!("expected one implementation PR, found {n}")),
    }
}

fn verify_correlated_engineer_pr(
    pr: &PullRequest,
    issue: ItemNumber,
    engineer_user: &str,
) -> Result<(), String> {
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
    if pr.author_id != UserId::new(engineer_user.to_string()) {
        return Err(format!(
            "PR #{} authored by {:?}, not the engineer identity {:?}",
            pr.number, pr.author_id, engineer_user
        ));
    }
    Ok(())
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
    // settings go in the config file; secrets (forge admin token, the engineer's
    // role identity, the dummy LLM api-key) go in a companion `credentials.toml`
    // passed via `--credentials`. The fake-LLM base URL is a provider profile
    // (`[agent.providers.deepseek] url`) and the dummy DeepSeek key is its
    // api-key credential — the agent reads no LLM env.
    let config_path = workspace.join("run-config.toml");
    let config = format!(
        "schema_version = 1\n\
         [forge]\n\
         type = \"forgejo\"\n\
         url = \"{base_url}\"\n\
         admin = \"admin\"\n\
         ci_user = \"{ENGINEER}\"\n\
         [engine]\n\
         bind = \"127.0.0.1:0\"\n\
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
         max_iterations = 6\n\
         [agent.providers.deepseek]\n\
         url = \"{fake_llm_url}\"\n",
        base_url = server.base_url(),
        owner = provisioned.owner,
        name = provisioned.name,
        workflow = workflow_file.display(),
        workspace_root = workspace.join("run/agent-workspaces").display(),
        fake_llm_url = fake_llm_url,
    );
    std::fs::write(&config_path, config).expect("write run config");

    // Credentials file replacing the removed FORGEJO_* / TEMPER_FORGEJO_*_ENGINEER
    // env injection: the forge admin token (keyed by `[forge] admin`), the web-UI
    // CI-read pair (keyed by `[forge] ci_user`), and the engineer's per-role
    // identity (keyed by the role name).
    let credentials_path = workspace.join("run-credentials.toml");
    let credentials = format!(
        "schema_version = 1\n\
         [forge.users.admin]\n\
         token = \"{admin_token}\"\n\
         [forge.users.{ENGINEER}]\n\
         user = \"{eng_user}\"\n\
         email = \"{eng_email}\"\n\
         password = \"{eng_password}\"\n\
         token = \"{eng_token}\"\n\
         [agent.providers.deepseek]\n\
         type = \"api-key\"\n\
         key = \"sk-jig-test\"\n",
        admin_token = provisioned.admin_token,
        eng_user = engineer.user,
        eng_email = engineer.email,
        eng_password = engineer.password,
        eng_token = engineer.token,
    );
    std::fs::write(&credentials_path, credentials).expect("write run credentials");

    // Hermeticity: point HOME / XDG_* at an isolated, empty dir under the temp
    // workspace so the spawned daemon can never discover the developer's or CI
    // user's global ~/.config/temper/credentials.toml. With the explicit
    // --config the daemon already suppresses default discovery (issue #202), but
    // isolating HOME makes the test independent of the box it runs on.
    let fake_home = workspace.join("home");
    std::fs::create_dir_all(fake_home.join(".config")).expect("fake home creates");

    let log_file = log_file(log);
    let child = Command::new(env!("CARGO_BIN_EXE_temper"))
        .arg("daemon")
        .arg("--config")
        .arg(&config_path)
        .arg("--credentials")
        .arg(&credentials_path)
        .env("HOME", &fake_home)
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .env("XDG_STATE_HOME", fake_home.join(".local/state"))
        // Agent LLM: the dummy DeepSeek api-key credential and the fake-LLM base
        // URL now live in the config/credentials files (the agent reads no
        // provider env), so nothing LLM-related is injected here.
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
