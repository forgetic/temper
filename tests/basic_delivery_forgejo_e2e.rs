//! Basic-delivery live e2e: the Rust equivalent of
//! `examples/basic-delivery/run.sh`.
//!
//! The ignored test boots a throwaway copy of cached bare-admin Forgejo state
//! plus a real host-mode `forgejo-runner`, drives real built `temper` binaries
//! (`init --apply` and `serve standalone`), uses the canonical basic-delivery
//! workflow bytes, and points the agent at an in-process jig-compatible fake
//! LLM. It files one thin site-admin intake issue and asserts the full path:
//!
//! raw intake webhook → mechanical `untriaged` → architect `ready_code` triage
//! → engineer product diff PR → real Forgejo Actions CI green → mechanical
//! merge → closed source issue.
//!
//! Run with:
//!   cargo test --test basic_delivery_forgejo_e2e -- --ignored --nocapture

#![cfg(unix)]

#[path = "basic_delivery_forgejo_e2e/convergence.rs"]
mod convergence;
#[path = "support/e2e_lock.rs"]
mod e2e_lock;
#[path = "basic_delivery_forgejo_e2e/fake_llm.rs"]
mod fake_llm;
#[path = "basic_delivery_forgejo_e2e/process.rs"]
mod process;

use std::time::{Duration, Instant};

use fake_llm::BasicDeliveryFake;
use process::{RunWorkspaceGuard, free_port, mint_site_admin_token, populate_repo};
use temper_testing::forgejo_server::{ForgejoRunner, start_cached_bare_admin_server};

const OWNER: &str = "acme";
const NAME: &str = "service";
const ENGINEER: &str = "engineer";

const ADMIN_USER: &str = "basicadmin";
const ADMIN_PASSWORD: &str = "Basic-Delivery-Admin-1!";
const INIT_PROVIDER_KEY: &str = "basic-delivery-jig-dummy-key";

const INTAKE_TITLE: &str = "Service banner should identify the environment";
const INTAKE_BODY: &str = include_str!("../examples/basic-delivery/config/intake-issue.md");
const EXAMPLE_CI: &str = include_str!("../examples/basic-delivery/config/ci.yml");

const EXAMPLE_WORKFLOW: &str = include_str!("../examples/basic-delivery/config/workflow.json");
const FIXTURE_WORKFLOW: &str =
    include_str!("../crates/temper-workflow/fixtures/basic-delivery.json");

const DAEMON_POLL_CADENCE_SECS: u64 = 600;
const MECHANICAL_CADENCE_SECS: u64 = 1;
const DEFAULT_CONVERGENCE_SECS: u64 = 360;

#[test]
#[ignore = "boots real Forgejo + forgejo-runner and spawns `temper` binaries; run with --ignored"]
fn basic_delivery_run_sh_equivalent_converges() {
    let _e2e_lock = e2e_lock::acquire();
    assert_canonical_workflow_bytes();
    let started = Instant::now();

    let cached =
        start_cached_bare_admin_server(ADMIN_USER, ADMIN_PASSWORD, "basicadmin@example.invalid")
            .expect("cached bare-admin Forgejo starts");
    let server = cached.server;
    let mut runner = ForgejoRunner::register(&server).expect("forgejo-runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");
    let admin_token = mint_site_admin_token(&server);
    eprintln!(
        "basic_delivery_forgejo_e2e world up: cache_hit={} runner={} startup={:?} forge={}",
        cached.cache_hit,
        runner.is_running(),
        started.elapsed(),
        server.base_url()
    );

    let fake = BasicDeliveryFake::start();
    let workspace = RunWorkspaceGuard::new("temper-basic-delivery-e2e");
    let bind_port = free_port();
    let bundle_dir = workspace.0.dir("bundle");
    let workspaces_dir = workspace.0.dir("workspaces");

    let init_log = workspace.0.join("logs/init.log");
    process::run_temper_init(
        &server,
        &bundle_dir,
        &workspaces_dir,
        bind_port,
        &fake.base_url(),
        &init_log,
    );
    let workflow_path = bundle_dir.join("workflow.yaml");
    assert_init_workflow_yaml_matches(&workflow_path);
    process::tune_init_config(
        &bundle_dir.join("config.toml"),
        DAEMON_POLL_CADENCE_SECS,
        MECHANICAL_CADENCE_SECS,
    );

    let populate_log = workspace.0.join("logs/repo-populate.log");
    populate_repo(
        server.base_url(),
        &admin_token,
        workspace.0.path(),
        &populate_log,
    );

    let run_log = workspace.0.join("logs/standalone.log");
    let mut standalone = process::spawn_temper_standalone(&bundle_dir, &run_log);
    process::wait_for_standalone(&mut standalone);

    let forge = convergence::admin_forge(server.base_url(), &admin_token);
    let repository = block_on(convergence::repository(&forge)).expect("repository resolves");
    let issue =
        block_on(convergence::seed_intake(&forge, &repository)).expect("intake issue seeds");
    eprintln!("basic_delivery_forgejo_e2e: seeded intake issue #{issue}");

    let timeout = convergence_timeout();
    let convergence_start = Instant::now();
    let result = convergence::drive_full_basic_delivery(
        &forge,
        &repository,
        issue,
        &mut standalone,
        timeout,
    );
    let convergence = convergence_start.elapsed();

    if let Err(error) = result {
        let runner_running = runner.is_running();
        panic!(
            "basic_delivery_forgejo_e2e did not converge within {timeout:?}: {error}\n\
             runner running={runner_running}, runner log tail:\n{}\n\
             --- init log ---\n{}\n--- repo populate log ---\n{}\n\
             --- standalone daemon/worker/agent log ---\n{}\n\
             --- fake LLM request tail ---\n{}\n--- CI diagnostics ---\n{}\n\
             --- Forgejo web log ---\n{}",
            runner.log_tail(),
            process::read_tail(&init_log, 120),
            process::read_tail(&populate_log, 120),
            standalone.log_tail(),
            fake.log_tail(),
            convergence::ci_diagnostics(&forge, &repository),
            process::read_tail(&server.data_dir().join("web.log"), 80),
        );
    }

    assert!(
        convergence < Duration::from_secs(DAEMON_POLL_CADENCE_SECS),
        "converged in {convergence:?}, not before the long poll backstop; raw webhooks should wake the standalone engine\n--- standalone log ---\n{}",
        standalone.log_tail()
    );
    assert!(
        fake.architect_requests() >= 2,
        "fake LLM never served the architect tool loop\n{}",
        fake.log_tail()
    );
    assert!(
        fake.engineer_requests() >= 2,
        "fake LLM never served the engineer tool loop\n{}",
        fake.log_tail()
    );

    eprintln!(
        "basic_delivery_forgejo_e2e: converged in {convergence:?} (total {:?})",
        started.elapsed()
    );
    let _ = standalone.child.kill();
}

fn assert_init_workflow_yaml_matches(path: &std::path::Path) {
    let workflow = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("init workflow {} is readable: {error}", path.display()));
    let generated_spec = temper_reference_delivery::parse_workflow_spec(path, &workflow)
        .expect("init workflow parses as YAML");
    let generated = generated_spec.validate().expect("init workflow validates");
    let validated = temper_reference_delivery::basic_delivery_workflow();
    assert_eq!(
        generated, validated,
        "temper init must write the canonical basic-delivery workflow"
    );
    assert!(
        !workflow.trim_start().starts_with('{'),
        "temper init should write workflow.yaml as YAML, not JSON bytes: {workflow}"
    );
}

fn assert_canonical_workflow_bytes() {
    assert_eq!(
        EXAMPLE_WORKFLOW, FIXTURE_WORKFLOW,
        "examples/basic-delivery/config/workflow.json must stay byte-equal to the workflow fixture"
    );
    assert_eq!(
        EXAMPLE_WORKFLOW,
        temper_reference_delivery::basic_delivery_workflow_json(),
        "embedded basic-delivery workflow must stay byte-equal to the example fixture"
    );
}

fn convergence_timeout() -> Duration {
    std::env::var("TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_CONVERGENCE_SECS))
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    temper_engine_io::build_runtime()
        .expect("engine runtime builds")
        .block_on(future)
}
