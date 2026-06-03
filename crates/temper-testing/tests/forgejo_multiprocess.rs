//! The Forgejo twin of `tests/multiprocess.rs` (Phase 4 of
//! `plans/forgejo-e2e/README.md`).
//!
//! Same one-process-per-part rehearsal as the filesystem `multiprocess` test, but
//! against a **real Forgejo server** with a **real host-mode `forgejo-runner`**
//! producing genuine CI. For each of the five reference-delivery scenarios it:
//!
//! 1. boots a throwaway [`ForgejoServer`] + [`ForgejoRunner`] and provisions the
//!    org, repo (`auto_init`), per-role users/tokens/passwords, labels, and the
//!    CI workflow ([`provision`]);
//! 2. seeds via the scenario's **exact** seed closure against an admin
//!    [`ForgejoForge`] — the same closure the filesystem test uses;
//! 3. spawns the `temper-testing-worker` binary `--backend forgejo --clock wall`
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
//! `#[ignore]`d, so the default `cargo test` never downloads a binary, opens a
//! socket, or spawns a server. No extra environment variable is required; run it
//! with:
//!
//! ```sh
//! cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
//! ```
//!
//! A real host CI job takes seconds and provisioning + git + HTTP add up, so the
//! convergence timeout, run-secs backstop, and poll cadence are **much** larger
//! than the filesystem test's. Each scenario boots its own server + runner for
//! full isolation (the simplest correct alternative to one-server-many-repos,
//! given provisioning targets a fixed repo).

use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_runner::Scenario;
use temper_testing::forgejo_server::{provision, ForgejoRunner, ForgejoServer, Provisioned};
#[allow(dead_code)]
#[path = "support/forgejo_multi_repo.rs"]
mod multi_repo_support;
#[path = "support/forgejo_multiprocess.rs"]
mod multiprocess_support;

use multiprocess_support::{convergence_timeout, WorkerFleet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use temper_testing::runner_config;
use temper_testing::scenarios::{
    changes_requested_then_approved, ci_fails_then_passes, cross_repo_fanout_converges,
    dependency_chain_mechanically_unblocked, happy_path,
};

/// How often the driver re-runs the assert closure while polling.
const ASSERT_POLL: Duration = Duration::from_secs(2);
/// Worker poll cadence; modest because every tick is real HTTP (findings-phase-0
/// §6 measured ~10–20 ms/call locally).
const WORKER_POLL_MS: u64 = 500;
/// Backstop run length per child, in case the driver dies before stopping it.
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
    /// Whether this variant provisions a second repo and passes both repos to
    /// the one fixed worker fleet.
    multi_repo: bool,
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn happy_path_converges_against_real_forgejo() {
    run_variant(&Variant {
        scenario: happy_path,
        architect: "default",
        reviewer: "default",
        ci_sentinel: "present",
        multi_repo: false,
    });
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn changes_requested_then_approved_converges_against_real_forgejo() {
    run_variant(&Variant {
        scenario: changes_requested_then_approved,
        architect: "default",
        reviewer: "request-changes-then-approve",
        ci_sentinel: "present",
        multi_repo: false,
    });
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn ci_fails_then_passes_converges_against_real_forgejo() {
    run_variant(&Variant {
        scenario: ci_fails_then_passes,
        architect: "default",
        reviewer: "default",
        // The PR head omits `ci-ok` so the first run fails; the engineer's fix
        // commit adds it, producing a second, passing head SHA (two verdicts).
        ci_sentinel: "deferred",
        multi_repo: false,
    });
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn dependency_chain_mechanically_unblocked_against_real_forgejo() {
    run_variant(&Variant {
        scenario: dependency_chain_mechanically_unblocked,
        architect: "closing",
        reviewer: "default",
        ci_sentinel: "present",
        multi_repo: false,
    });
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn cross_repo_fanout_converges_against_real_forgejo() {
    run_variant(&Variant {
        scenario: cross_repo_fanout_converges,
        architect: "closing",
        reviewer: "default",
        ci_sentinel: "present",
        multi_repo: true,
    });
}

/// Drives one scenario through the true multi-process topology against a real
/// Forgejo, asserting it converges to the same end state as the in-process
/// scenario.
fn run_variant(variant: &Variant) {
    // Boot the server + runner. `ForgejoServer::start` polls readiness with a
    // *blocking* reqwest client whose nested runtime must live and die off any
    // async reactor — this driver is a plain `#[test]`, so there is no reactor to
    // collide with, and the worker processes carry their own Tokio runtimes.
    let server = ForgejoServer::start().expect("forgejo server boots");
    let mut runner = ForgejoRunner::register(&server).expect("forgejo runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");

    let provisioned = block_on_provision(&server);
    let mut repo_args = vec![format!("{}/{}", provisioned.owner, provisioned.name)];
    if variant.multi_repo {
        let second_name = "service-canary".to_string();
        multi_repo_support::futures_block_on(multi_repo_support::provision_extra_repo(
            &server,
            &provisioned,
            &second_name,
        ))
        .expect("second Forgejo repo provisions for cross-repo scenario");
        repo_args.push(format!("{}/{}", provisioned.owner, second_name));
    }
    let config = runner_config();

    // Seed via the scenario's exact seed closure against an admin-owner backend.
    let scenario = (variant.scenario)();
    seed(&server, &provisioned, &scenario);

    let stop_file = server_stop_file(&provisioned);
    let _ = std::fs::remove_file(&stop_file);

    // Spawn one process per moving part behind a kill-on-drop guard.
    let mut workers = WorkerFleet::spawn(
        &server,
        &provisioned,
        &repo_args,
        &stop_file,
        &config,
        variant.architect,
        variant.reviewer,
        variant.ci_sentinel,
    );

    // Detect convergence in-process by polling the exact scenario assert.
    let timeout = convergence_timeout();
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
fn any_role(provisioned: &Provisioned) -> Option<&temper_testing::forgejo_server::RoleIdentity> {
    runner_config()
        .role_bindings
        .iter()
        .find_map(|binding| provisioned.role(&binding.role))
}

/// Lists each PR, its head branch, and its CI jobs for a convergence-failure
/// message. Best-effort: any read error is folded into the string.
fn ci_diagnostics(server: &ForgejoServer, provisioned: &Provisioned) -> String {
    use temper_forge::{CiJobQuery, PullRequestQuery};
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
        "temper-forgejo-mp-stop-{}-{}-{id}",
        provisioned.name,
        std::process::id()
    ))
}

static NEXT_STOP: AtomicU64 = AtomicU64::new(0);

fn touch(path: &Path) {
    std::fs::write(path, b"stop").expect("writing the stop sentinel succeeds");
}
