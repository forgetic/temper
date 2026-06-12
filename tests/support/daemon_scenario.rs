//! Shared driver for the daemon-topology real-Forgejo e2e scenarios.
//!
//! Topology under test (mirrors the production cutover):
//!
//! - `ForgejoServer` + host-mode `ForgejoRunner` from the shared bench fixture
//!   (real Forgejo API, real git, real Actions CI),
//! - the real **`temper-daemon` binary** (`CARGO_BIN_EXE_temper-daemon`):
//!   env→config→composition, webhook route, long poll backstop, short
//!   mechanical backstop, per-role token routing,
//! - the deterministic **`temper-testing-daemon-worker`** binary: wire-protocol
//!   client + real git push as the engineer role identity.
//!
//! The daemon runs the engineer-only daemon-delivery workflow
//! (`daemon-delivery.json`, the dogfood deployment shape): the mechanical
//! `raw_intake` automation stamps the seeded intake issue `code`+`ready`, the
//! engineer worker pushes the implementation branch, the daemon applies the
//! result as the engineer (PR authored by the engineer role), and the
//! mechanical backstop lands the PR once real CI is green. Merging closes the
//! source issue through the provider's native close-on-merge keyword carried by
//! the worker's commit.
//!
//! The daemon's poll backstop is deliberately long: webhooks must drive all
//! Forge-event progress before that deadline (CI completion has no webhook on
//! Forgejo 7.0.x, which is exactly what the short mechanical cadence backstops,
//! same as the legacy fleet's narrow CI status poll).

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use temper_forge::{
    CiJob, CiJobConclusion, CiJobQuery, CiJobStatus, IssueState, ItemNumber, PullRequest,
    PullRequestQuery, PullRequestState, UserId,
};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_testing::forgejo_runtime::RunWorkspace;
use temper_testing::forgejo_server::{
    commit_ci_sentinel, seed_intake_issue, start_cached_provisioned_repositories, ForgejoRunner,
    ForgejoServer, Provisioned, RoleIdentity,
};
use temper_workflow::{parse_metadata_block, CiStatus, RoleId};

/// The engineer-only daemon-delivery workflow served to the daemon binary via
/// `--workflow` (the dogfood deployment shape: mechanical `mark_ready` intake
/// automation + engineer `open_pr` + mechanical CI-gated `land_pr`).
const DAEMON_WORKFLOW: &str = include_str!("daemon-delivery.json");

/// How often the driver re-runs the assert closure while polling.
const ASSERT_POLL: Duration = Duration::from_secs(1);
/// Deliberately long daemon poll backstop; scenario progress must come from
/// Forgejo webhooks delivered to the daemon's webhook route before this
/// deadline.
const DAEMON_POLL_CADENCE_SECS: u64 = 600;
/// Narrow mechanical backstop cadence. Forgejo 7.0.x does not emit
/// Actions-completion webhooks through repository hooks, so mechanical landing
/// keeps a short poll for CI status transitions only (legacy `CI_STATUS_POLL`).
const DAEMON_MECHANICAL_CADENCE_SECS: u64 = 2;
const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(300);
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(60);

const WEBHOOK_SECRET: &str = "daemon-e2e-webhook-secret";
const ENGINEER: &str = "engineer";

pub fn convergence_timeout() -> Duration {
    std::env::var("TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(CONVERGENCE_TIMEOUT)
}

/// One daemon-topology scenario.
#[derive(Clone, Copy)]
pub struct Variant {
    /// Stable name used in logs, repo names, and panic messages.
    pub name: &'static str,
    /// Fresh primary repository for this scenario.
    pub repo_name: &'static str,
    /// `--ci-sentinel` value passed to the daemon test worker.
    pub ci_sentinel: &'static str,
}

impl Variant {
    pub fn happy_path() -> Self {
        Self {
            name: "happy_path",
            repo_name: "daemon-happy-path",
            ci_sentinel: "present",
        }
    }

    pub fn ci_fails_then_passes() -> Self {
        Self {
            name: "ci_fails_then_passes",
            repo_name: "daemon-ci-retry",
            ci_sentinel: "deferred",
        }
    }
}

pub fn run_daemon_variant(variant: Variant) {
    let test_start = Instant::now();

    // World: cached provisioned Forgejo (org, role identities, labels, repo,
    // Actions + the marker-gated CI workflow) plus the host-mode runner.
    let cached = start_cached_provisioned_repositories(&[variant.repo_name.to_string()])
        .expect("forgejo cached provisioned scenario state starts");
    let server = cached.server;
    let mut runner = ForgejoRunner::register(&server).expect("forgejo runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");
    let provisioned = cached
        .state
        .provisioned(variant.repo_name)
        .unwrap_or_else(|| panic!("cached state has no repo named {}", variant.repo_name));
    let engineer = provisioned
        .role(&RoleId::new(ENGINEER))
        .expect("engineer identity is provisioned")
        .clone();
    eprintln!(
        "daemon_forgejo_e2e scenario '{}' world up: cache_hit={} startup={:?}",
        variant.name,
        cached.cache_hit,
        test_start.elapsed()
    );

    let workspace = RunWorkspaceGuard::new("temper-daemon-forgejo-e2e");
    let secret_file = workspace
        .0
        .write_file("daemon/webhook-secret", format!("{WEBHOOK_SECRET}\n"));
    let workflow_file = workspace
        .0
        .write_file("daemon/workflow.json", DAEMON_WORKFLOW);

    // Real temper-daemon binary on a free local port.
    let port = free_port();
    let daemon_log = workspace.0.join("daemon/daemon.log");
    let mut daemon = spawn_daemon(
        &server,
        &provisioned,
        &engineer,
        port,
        &workflow_file,
        &secret_file,
        &daemon_log,
    );
    wait_for_daemon(port, &mut daemon);

    register_webhook(&server, &provisioned, port);

    // Deterministic wire-protocol worker with the engineer git identity.
    let stop_file = workspace.0.join("worker/stop");
    workspace.0.dir("worker");
    let worker_log = workspace.0.join("worker/worker.log");
    let mut worker = spawn_worker(
        &server,
        &provisioned,
        &engineer,
        port,
        workspace.0.path(),
        &stop_file,
        variant.ci_sentinel,
        &worker_log,
    );

    // Seed one raw intake issue; the daemon's mechanical backstop stamps it
    // code+ready and the resulting label webhook wakes the engineer feed.
    let issue = block_on(seed_intake_issue(
        server.base_url(),
        &provisioned.admin_token,
        &provisioned.owner,
        &provisioned.name,
    ))
    .expect("intake issue seeds");
    eprintln!(
        "daemon_forgejo_e2e scenario '{}' seeded intake issue #{issue}",
        variant.name
    );

    let timeout = convergence_timeout();
    let convergence_start = Instant::now();
    let forge = admin_forge(&server, &provisioned, &engineer);

    let converged = drive_variant(
        &variant,
        &server,
        &provisioned,
        &engineer,
        &forge,
        issue,
        timeout,
    );
    let convergence = convergence_start.elapsed();

    if let Err(error) = converged {
        let runner_running = runner.is_running();
        panic!(
            "scenario '{}' did not converge within {timeout:?}: {error}\n\
             runner running={runner_running}, runner log tail:\n{}\n\
             --- daemon log ---\n{}\n--- worker log ---\n{}\n--- CI diagnostics ---\n{}",
            variant.name,
            runner.log_tail(),
            daemon.log_tail(),
            worker.log_tail(),
            ci_diagnostics(&forge, &provisioned)
        );
    }

    // Webhooks (plus the narrow mechanical CI backstop) must have driven
    // convergence; the long poll backstop never gets a chance to.
    assert!(
        convergence < Duration::from_secs(DAEMON_POLL_CADENCE_SECS),
        "scenario '{}' converged in {convergence:?}, not before the {DAEMON_POLL_CADENCE_SECS}s poll backstop\n--- daemon log ---\n{}",
        variant.name,
        daemon.log_tail()
    );

    // Graceful worker shutdown through the stop-file; the daemon is killed by
    // its drop guard (it has no stop seam, mirroring production systemd stop).
    std::fs::write(&stop_file, b"stop").expect("stop file writes");
    let status = worker.wait(Duration::from_secs(30));
    assert!(
        status.success(),
        "scenario '{}' worker exited with {status:?}\n--- worker log ---\n{}",
        variant.name,
        worker.log_tail()
    );

    eprintln!(
        "daemon_forgejo_e2e scenario '{}' converged in {convergence:?} (total {:?})",
        variant.name,
        test_start.elapsed()
    );
}

fn drive_variant(
    variant: &Variant,
    server: &ForgejoServer,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
    forge: &ForgejoForge,
    issue: ItemNumber,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    match variant.ci_sentinel {
        "present" => poll_until(deadline, || {
            block_on(assert_converged(forge, provisioned, engineer, issue, 1))
        }),
        "deferred" => {
            // Phase A: the worker's marker-less head fails real CI and the PR
            // must stay unmerged while red.
            let head_branch = poll_until(deadline, || {
                block_on(assert_ci_red_and_unmerged(
                    forge,
                    provisioned,
                    engineer,
                    issue,
                ))
            })?;

            // Phase B: push the CI sentinel fix to the PR head as the engineer;
            // the new head goes green and the mechanical backstop lands it.
            {
                let base_url = server.base_url().to_string();
                let token = engineer.token.clone();
                let owner = provisioned.owner.clone();
                let name = provisioned.name.clone();
                let branch = head_branch.clone();
                block_on_with_cx(move |cx| async move {
                    commit_ci_sentinel(&cx, &base_url, &token, &owner, &name, &branch).await
                })
            }
            .map_err(|error| format!("ci sentinel commit failed: {error}"))?;
            eprintln!(
                "daemon_forgejo_e2e scenario '{}' pushed CI sentinel fix to {head_branch}",
                variant.name
            );

            poll_until(deadline, || {
                block_on(assert_converged(forge, provisioned, engineer, issue, 2))
            })
        }
        other => panic!("unknown ci_sentinel variant '{other}'"),
    }
}

/// Full convergence: one merged engineer-authored implementation PR correlated
/// to the seeded issue, green real CI, and the source issue closed.
async fn assert_converged(
    forge: &ForgejoForge,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
    issue: ItemNumber,
    min_ci_verdicts: usize,
) -> Result<(), String> {
    let pull_request = implementation_pr(forge, provisioned, issue).await?;

    if pull_request.author_id != UserId::new(engineer.user.clone()) {
        return Err(format!(
            "implementation PR #{} was authored by {:?}, not the engineer role identity",
            pull_request.number, pull_request.author_id
        ));
    }

    if pull_request.state != PullRequestState::Merged {
        return Err(format!(
            "implementation PR #{} is not merged (state {:?})",
            pull_request.number, pull_request.state
        ));
    }
    let merge = pull_request
        .merge
        .as_ref()
        .ok_or("merged implementation PR has no merge record")?;
    if merge.merged_by == UserId::new(engineer.user.clone()) {
        return Err(
            "PR was merged by the engineer role identity, not the daemon's mechanical backstop"
                .to_string(),
        );
    }
    if !pull_request.labels.iter().any(|label| label == "landed") {
        return Err("merged implementation PR is missing the landed label".to_string());
    }

    let jobs = completed_ci_jobs(forge, provisioned, &pull_request).await?;
    if jobs.len() < min_ci_verdicts {
        return Err(format!(
            "expected at least {min_ci_verdicts} completed CI verdicts, found {}",
            jobs.len()
        ));
    }
    if min_ci_verdicts >= 2
        && jobs.first().and_then(|job| job.conclusion) != Some(CiJobConclusion::Failure)
    {
        return Err("first CI verdict did not fail".to_string());
    }
    if jobs.last().and_then(|job| job.conclusion) != Some(CiJobConclusion::Success) {
        return Err("latest CI verdict did not pass".to_string());
    }
    if !CiStatus::from_jobs(&jobs).is_passed() {
        return Err("latest CI aggregate is not passing".to_string());
    }

    let issue = forge
        .get_issue_by_number(&provisioned.repository, issue)
        .await
        .map_err(|error| format!("issue lookup failed: {error}"))?
        .ok_or("source issue disappeared")?;
    if issue.state != IssueState::Closed {
        return Err(format!(
            "source issue #{} was not closed on merge (labels {:?})",
            issue.number, issue.labels
        ));
    }

    Ok(())
}

/// Red phase of the CI variant: the engineer-authored PR exists, has at least
/// one failed real CI verdict, and is **not** merged. Returns the head branch.
async fn assert_ci_red_and_unmerged(
    forge: &ForgejoForge,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
    issue: ItemNumber,
) -> Result<String, String> {
    let pull_request = implementation_pr(forge, provisioned, issue).await?;

    if pull_request.author_id != UserId::new(engineer.user.clone()) {
        return Err(format!(
            "implementation PR #{} was authored by {:?}, not the engineer role identity",
            pull_request.number, pull_request.author_id
        ));
    }
    assert!(
        pull_request.merge.is_none() && pull_request.state == PullRequestState::Open,
        "implementation PR #{} merged or closed while its CI was red (state {:?})",
        pull_request.number,
        pull_request.state
    );

    let jobs = completed_ci_jobs(forge, provisioned, &pull_request).await?;
    if jobs.last().and_then(|job| job.conclusion) != Some(CiJobConclusion::Failure) {
        return Err(format!(
            "no failed CI verdict for PR #{} yet ({} completed jobs)",
            pull_request.number,
            jobs.len()
        ));
    }

    Ok(pull_request.source.branch.clone())
}

/// Finds the single implementation PR and checks its workflow correlation
/// metadata points at the seeded issue.
async fn implementation_pr(
    forge: &ForgejoForge,
    provisioned: &Provisioned,
    issue: ItemNumber,
) -> Result<PullRequest, String> {
    let pull_requests: Vec<PullRequest> = forge
        .list_pull_requests(&provisioned.repository, PullRequestQuery::default())
        .await
        .map_err(|error| format!("list_pull_requests failed: {error}"))?
        .into_iter()
        .filter(|pull_request| {
            pull_request
                .labels
                .iter()
                .any(|label| label == "implementation")
        })
        .collect();
    if pull_requests.len() != 1 {
        return Err(format!(
            "expected exactly one implementation PR, found {}",
            pull_requests.len()
        ));
    }
    let pull_request = pull_requests.into_iter().next().expect("one PR");

    let metadata = parse_metadata_block(&pull_request.body)
        .map_err(|error| format!("implementation PR metadata is malformed: {error}"))?
        .ok_or("implementation PR is missing workflow metadata")?;
    let expected_key = format!("pr-for-code-{issue}");
    if metadata.correlation_key.as_deref() != Some(expected_key.as_str()) {
        return Err(format!(
            "implementation PR correlation key {:?} != {expected_key:?}",
            metadata.correlation_key
        ));
    }
    if !metadata
        .parents
        .iter()
        .any(|parent| parent.is_same_repo() && parent.number == issue)
    {
        return Err(format!(
            "implementation PR parents {:?} do not include issue #{issue}",
            metadata.parents
        ));
    }

    Ok(pull_request)
}

async fn completed_ci_jobs(
    forge: &ForgejoForge,
    provisioned: &Provisioned,
    pull_request: &PullRequest,
) -> Result<Vec<CiJob>, String> {
    let mut jobs = forge
        .list_ci_jobs(
            &provisioned.repository,
            CiJobQuery {
                pull_request_id: Some(pull_request.id.clone()),
                status: Some(CiJobStatus::Completed),
                ..CiJobQuery::default()
            },
        )
        .await
        .map_err(|error| format!("list_ci_jobs failed: {error}"))?;
    jobs.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then(left.id.cmp(&right.id))
    });
    Ok(jobs)
}

/// Admin-token forge handle with the engineer's web-UI credentials attached for
/// the ADR 0019 CI reads, mirroring the daemon binary's own environment.
fn admin_forge(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
) -> ForgejoForge {
    ForgejoForge::new(
        ForgejoConfig::new(server.base_url(), &provisioned.admin_token)
            .with_default_repo(&provisioned.owner, &provisioned.name)
            .with_web_ui_credentials(&engineer.user, &engineer.password),
    )
}

fn spawn_daemon(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
    port: u16,
    workflow_file: &Path,
    secret_file: &Path,
    log: &Path,
) -> ChildGuard {
    let log_file = log_file(log);
    let child = Command::new(env!("CARGO_BIN_EXE_temper-daemon"))
        .arg("--bind")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--repo")
        .arg(format!("{}/{}", provisioned.owner, provisioned.name))
        .arg("--role")
        .arg(ENGINEER)
        .arg("--workflow")
        .arg(workflow_file)
        .arg("--webhook-secret-file")
        .arg(secret_file)
        .arg("--poll-cadence-secs")
        .arg(DAEMON_POLL_CADENCE_SECS.to_string())
        .arg("--mechanical-cadence-secs")
        .arg(DAEMON_MECHANICAL_CADENCE_SECS.to_string())
        .arg("--daemon-id")
        .arg("temper-daemon-e2e")
        .env("FORGEJO_URL", server.base_url())
        .env("FORGEJO_ACCESS_TOKEN", &provisioned.admin_token)
        // ADR 0019 web-UI CI-read fallback used by the mechanical backstop.
        .env("FORGEJO_USERNAME", &engineer.user)
        .env("FORGEJO_PASSWORD", &engineer.password)
        // Per-role Forge API token routing (consolidation phase 4d).
        .env("TEMPER_FORGEJO_TOKEN_ENGINEER", &engineer.token)
        .env_remove("FORGEJO_DEFAULT_REPO")
        .stdout(Stdio::from(
            log_file.try_clone().expect("daemon log clones"),
        ))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("temper-daemon binary spawns");
    ChildGuard {
        label: "temper-daemon",
        child,
        log: log.to_path_buf(),
    }
}

/// Waits until the daemon accepts TCP connections on its bind port. Startup
/// includes Forge repository resolution and workflow label upserts, so this can
/// take a few seconds on a fresh world.
fn wait_for_daemon(port: u16, daemon: &mut ChildGuard) {
    let deadline = Instant::now() + DAEMON_READY_TIMEOUT;
    loop {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if let Some(status) = daemon.child.try_wait().expect("daemon try_wait") {
            panic!(
                "temper-daemon exited during startup with {status:?}\n--- daemon log ---\n{}",
                daemon.log_tail()
            );
        }
        assert!(
            Instant::now() < deadline,
            "temper-daemon did not bind 127.0.0.1:{port} within {DAEMON_READY_TIMEOUT:?}\n--- daemon log ---\n{}",
            daemon.log_tail()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_worker(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
    daemon_port: u16,
    workspace: &Path,
    stop_file: &Path,
    ci_sentinel: &str,
    log: &Path,
) -> ChildGuard {
    let log_file = log_file(log);
    let child = Command::new(daemon_worker_binary())
        .arg("--daemon-url")
        .arg(format!("http://127.0.0.1:{daemon_port}"))
        .arg("--worker-id")
        .arg("daemon-e2e-worker-1")
        .arg("--capability")
        .arg(format!(
            "{}/{}:{ENGINEER}",
            provisioned.owner, provisioned.name
        ))
        .arg("--git-base-url")
        .arg(server.base_url())
        .arg("--workspace-root")
        .arg(workspace.join("worker/root"))
        .arg("--stop-file")
        .arg(stop_file)
        .arg("--ci-sentinel")
        .arg(ci_sentinel)
        .env("TEMPER_E2E_GIT_USER", &engineer.user)
        .env("TEMPER_E2E_GIT_TOKEN", &engineer.token)
        .stdout(Stdio::from(
            log_file.try_clone().expect("worker log clones"),
        ))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("temper-testing-daemon-worker binary spawns");
    ChildGuard {
        label: "temper-testing-daemon-worker",
        child,
        log: log.to_path_buf(),
    }
}

/// Resolves the daemon test worker binary, which lives in the `temper-testing`
/// package (so no `CARGO_BIN_EXE_…` is exported to this root-package test).
/// Resolution order: explicit env override, then the workspace target dir next
/// to the daemon binary, building it on demand when absent.
fn daemon_worker_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("TEMPER_TESTING_DAEMON_WORKER_BIN") {
        return PathBuf::from(path);
    }
    let daemon = PathBuf::from(env!("CARGO_BIN_EXE_temper-daemon"));
    let candidate = daemon
        .parent()
        .expect("daemon binary has a target directory")
        .join("temper-testing-daemon-worker");
    if !candidate.exists() {
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "-j2",
                "-p",
                "temper-testing",
                "--bin",
                "temper-testing-daemon-worker",
            ])
            .status()
            .expect("cargo build for the daemon test worker runs");
        assert!(
            status.success(),
            "building temper-testing-daemon-worker failed"
        );
        assert!(
            candidate.exists(),
            "temper-testing-daemon-worker missing at {} after build; set TEMPER_TESTING_DAEMON_WORKER_BIN",
            candidate.display()
        );
    }
    candidate
}

fn register_webhook(server: &ForgejoServer, provisioned: &Provisioned, port: u16) {
    let url = format!("http://127.0.0.1:{port}/forgejo/webhook");
    let base_url = server.base_url().to_string();
    let admin_token = provisioned.admin_token.clone();
    let owner = provisioned.owner.clone();
    let name = provisioned.name.clone();
    block_on_with_cx(move |cx| async move {
        temper_forgejo_ops::forgejo_rest::ensure_repo_webhook(
            &temper_forgejo_ops::forgejo_rest::http_client(cx.clone()).expect("HTTP client builds"),
            &base_url,
            &admin_token,
            &owner,
            &name,
            &url,
            WEBHOOK_SECRET,
        )
        .await
    })
    .unwrap_or_else(|error| {
        panic!(
            "repo webhook registration failed for {}/{}: {error}",
            provisioned.owner, provisioned.name
        )
    });
}

/// Lists each PR, its head, and its CI jobs for convergence-failure messages.
fn ci_diagnostics(forge: &ForgejoForge, provisioned: &Provisioned) -> String {
    block_on(async {
        let mut out = String::new();
        match forge
            .list_pull_requests(&provisioned.repository, PullRequestQuery::default())
            .await
        {
            Ok(pull_requests) => {
                for pull_request in &pull_requests {
                    out.push_str(&format!(
                        "PR #{} head={} labels={:?} state={:?} merge={}\n",
                        pull_request.number,
                        pull_request.source.branch,
                        pull_request.labels,
                        pull_request.state,
                        pull_request.merge.is_some()
                    ));
                    match forge
                        .list_ci_jobs(
                            &provisioned.repository,
                            CiJobQuery {
                                pull_request_id: Some(pull_request.id.clone()),
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

/// Polls `assert` until it passes or `deadline` elapses, returning the last
/// error on timeout.
fn poll_until<T>(
    deadline: Instant,
    mut assert: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    loop {
        let error = match assert() {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if Instant::now() >= deadline {
            return Err(error);
        }
        std::thread::sleep(ASSERT_POLL);
    }
}

/// Allocates a free local port via bind-then-drop.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("ephemeral port binds")
        .local_addr()
        .expect("bound listener has an address")
        .port()
}

fn log_file(path: &Path) -> std::fs::File {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("log dir creates");
    }
    std::fs::File::create(path).expect("log file creates")
}

/// Drives a single boxed future to completion on a one-shot engine runtime;
/// the real backend's futures park on network IO.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    temper_io_engine::build_runtime()
        .expect("engine runtime builds")
        .block_on(future)
}

/// [`block_on`] handing the body the root task's `Cx` (clock capability) for
/// code whose deadlines must be computed against the engine clock.
fn block_on_with_cx<F, Fut>(f: F) -> Fut::Output
where
    F: FnOnce(temper_io_engine::Cx) -> Fut + Send + 'static,
    Fut: std::future::Future + Send + 'static,
    Fut::Output: Send + 'static,
{
    let runtime = temper_io_engine::build_runtime().expect("engine runtime builds");
    temper_io_engine::runtime::block_on_runtime_with(&runtime, move |cx, _handle| f(cx))
}

/// Kills the spawned child on drop so a panic cannot leak daemon or worker
/// processes (ported from the legacy fleet support's drop discipline).
struct ChildGuard {
    label: &'static str,
    child: Child,
    log: PathBuf,
}

impl ChildGuard {
    fn log_tail(&self) -> String {
        match std::fs::read_to_string(&self.log) {
            Ok(contents) => {
                let lines: Vec<&str> = contents.lines().rev().take(120).collect();
                lines.into_iter().rev().collect::<Vec<_>>().join("\n")
            }
            Err(error) => format!("<could not read {}: {error}>", self.log.display()),
        }
    }

    /// Waits for a graceful exit, escalating to kill at `timeout`.
    fn wait(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("child try_wait") {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                return self.child.wait().expect("child waits after kill");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.label;
    }
}

/// Owns the run workspace; kept so its drop cleanup runs after the children's.
struct RunWorkspaceGuard(RunWorkspace);

impl RunWorkspaceGuard {
    fn new(prefix: &str) -> Self {
        Self(RunWorkspace::new(prefix))
    }
}
