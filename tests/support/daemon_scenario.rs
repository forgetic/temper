//! Shared driver for the daemon-topology real-Forgejo e2e scenarios.
//!
//! Topology under test (mirrors the production cutover):
//!
//! - `ForgejoServer` + host-mode `ForgejoRunner` from the shared bench fixture
//!   (real Forgejo API, real git, real Actions CI),
//! - the real engine service (`temper daemon --service engine`, from the
//!   `CARGO_BIN_EXE_temper` binary, config-file driven): webhook route, long
//!   poll backstop, short mechanical backstop, per-role token routing,
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

#[path = "daemon_scenario/convergence.rs"]
mod convergence;
#[path = "daemon_scenario/process.rs"]
mod process;
#[path = "daemon_scenario/runtime.rs"]
mod runtime;

use std::time::{Duration, Instant};

use process::{
    RunWorkspaceGuard, free_port, register_webhook, spawn_daemon, spawn_worker, wait_for_daemon,
};
use temper_testing::forgejo_server::{
    ForgejoRunner, seed_intake_issue, start_cached_provisioned_repositories,
};
use temper_workflow::RoleId;

/// The engineer-only daemon-delivery workflow served to the daemon binary via
/// `--workflow` (the dogfood deployment shape: mechanical `mark_ready` intake
/// automation + engineer `open_pr` + mechanical CI-gated `land_pr`).
const DAEMON_WORKFLOW: &str = include_str!("daemon-delivery.json");

/// Deliberately long daemon poll backstop; scenario progress must come from
/// Forgejo webhooks delivered to the daemon's webhook route before this
/// deadline.
const DAEMON_POLL_CADENCE_SECS: u64 = 600;
const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(300);

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
    let secret_file = workspace.0.write_file(
        "daemon/webhook-secret",
        format!("{}\n", process::WEBHOOK_SECRET),
    );
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
    let issue = runtime::block_on(seed_intake_issue(
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
    let forge = convergence::admin_forge(&server, &provisioned, &engineer);

    let converged = convergence::drive_variant(
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
            convergence::ci_diagnostics(&forge, &provisioned)
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
