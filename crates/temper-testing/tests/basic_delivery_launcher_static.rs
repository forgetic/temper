//! Static (source-level) assertions for the `examples/basic-delivery` launcher.
//!
//! Mirrors `reference_delivery_launcher_static.rs` at basic-delivery scope: it
//! reads `run.sh` / `config/temper.env` as text and asserts the invariants that
//! keep the no-human-in-the-loop demo correct and secret-safe — the mechanical
//! worker gets the `bot` credentials via the environment (never argv), the
//! bundled 3-role workflow and the basic agent selector reach both the provision
//! and the worker invocations, intake is seeded as the site admin, the ports are
//! distinct from reference-delivery, and the ADR-0019 CI read fallback knobs are
//! present. It also pins `config/workflow.json` byte-for-byte to the canonical
//! fixture so the two never drift.

use std::{fs, path::PathBuf};

fn example_path(relative: &str) -> PathBuf {
    workspace_root()
        .join("examples/basic-delivery")
        .join(relative)
}

fn workspace_root() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::current_dir() {
        candidates.push(path);
    }
    if let Some(value) = std::env::var_os("CARGO_MANIFEST_DIR") {
        if !value.is_empty() {
            candidates.push(PathBuf::from(value));
        }
    }
    candidates
        .into_iter()
        .find_map(|start| {
            start
                .ancestors()
                .find(|dir| {
                    dir.join("Cargo.toml").is_file() && dir.join("examples/basic-delivery").is_dir()
                })
                .map(PathBuf::from)
        })
        .expect("workspace root resolves")
}

fn read_example(relative: &str) -> String {
    fs::read_to_string(example_path(relative)).expect("example file is readable")
}

#[test]
fn mechanical_worker_gets_bot_credentials_without_argv_secrets() {
    let script = read_example("run.sh");
    let launch_workers = script
        .split("launch_workers() {")
        .nth(1)
        .expect("launch_workers function exists");
    let mechanical_spawn = launch_workers
        .split("TEMPER_FORGEJO_TOKEN=\"$BOT_TOKEN\"")
        .nth(1)
        .expect("mechanical worker uses the bot REST token")
        .split(") >\"$LOG_DIR/mechanical.log\"")
        .next()
        .expect("mechanical spawn stanza is bounded");

    assert!(mechanical_spawn.contains("TEMPER_FORGEJO_USERNAME=\"$BOT_USER\""));
    assert!(launch_workers.contains("ci_reader=bot"));
    assert!(mechanical_spawn.contains("TEMPER_FORGEJO_PASSWORD=\"$BOT_PASSWORD\""));
    assert!(mechanical_spawn.contains("--poll-ms \"$CI_STATUS_POLL_MS\""));
    assert!(mechanical_spawn.contains("--idle-poll-max-ms \"$IDLE_POLL_MAX_MS\""));
    assert!(!launch_workers.contains("TEMPER_FORGEJO_TOKEN=\"$ADMIN_TOKEN\""));

    let argv = mechanical_spawn
        .split("\"$TESTING_WORKER_BIN\"")
        .nth(1)
        .expect("worker argv follows the binary path");
    assert!(!argv.contains("BOT_PASSWORD"));
    assert!(!argv.contains("BOT_TOKEN"));
    assert!(!argv.contains("TEMPER_FORGEJO_PASSWORD"));
    assert!(!argv.contains("TEMPER_FORGEJO_TOKEN"));
    assert!(!argv.contains("--password"));
    assert!(!argv.contains("--token"));
}

#[test]
fn bot_identity_is_resolved_before_mechanical_worker_launch() {
    let script = read_example("run.sh");
    assert!(script.contains("automation user 'bot' has no username"));
    assert!(script.contains("automation user must be 'bot'"));
    assert!(script.contains("automation user 'bot' has no token"));
    assert!(script.contains("automation user 'bot' has no password"));

    let launch_workers = script
        .split("launch_workers() {")
        .nth(1)
        .expect("launch_workers function exists");
    let resolve_index = launch_workers
        .find("resolve_bot_identity")
        .expect("bot identity is resolved");
    let spawn_index = launch_workers
        .find("TEMPER_FORGEJO_TOKEN=\"$BOT_TOKEN\"")
        .expect("mechanical worker spawn exists");
    assert!(
        resolve_index < spawn_index,
        "bot credentials should fail before the mechanical worker process starts"
    );
}

#[test]
fn launcher_uses_non_reserved_setup_admin_handle() {
    let script = read_example("run.sh");

    // Forgejo reserves the literal username `admin`; the throwaway setup admin
    // uses a valid non-reserved handle.
    assert!(script.contains("ADMIN_USER=basicadmin"));
    assert!(!script.contains("ADMIN_USER=admin\n"));
    assert!(script.contains("Forgejo reserves the"));
}

#[test]
fn launcher_passes_workflow_and_basic_profile_to_provision_and_worker() {
    let script = read_example("run.sh");
    let config = read_example("config/temper.env");

    // W1 (#63): the bundled 3-role spec reaches BOTH the provision and the worker
    // invocations via --workflow "$WORKFLOW_PATH".
    assert!(config.contains("WORKFLOW_FILE=workflow.json"));
    assert!(script.contains("--workflow \"$WORKFLOW_PATH\""));

    let bootstrap = script
        .split("bootstrap_and_provision() {")
        .nth(1)
        .expect("bootstrap_and_provision function exists");
    assert!(
        bootstrap.contains("--workflow \"$WORKFLOW_PATH\""),
        "provision invocation must pass --workflow"
    );

    // W2 (#62): the role workers select the basic-delivery fake agent set. The
    // basic profile rides only on the role workers (the mechanical reconciler is
    // workflow-driven, not profile-driven).
    let launch_role_worker = script
        .split("launch_role_worker() {")
        .nth(1)
        .expect("launch_role_worker function exists")
        .split("\nlaunch_workers() {")
        .next()
        .expect("launch_role_worker is bounded before launch_workers");
    assert!(
        launch_role_worker.contains("--workflow \"$WORKFLOW_PATH\" --profile basic"),
        "role workers must pass --workflow and --profile basic"
    );
    assert!(launch_role_worker.contains("--agents fake"));
}

#[test]
fn launcher_seeds_intake_as_site_admin() {
    let script = read_example("run.sh");
    let workflow = read_example("config/workflow.json");

    // W3 (#65): the bundled spec declares intake_author = site_admin, so the
    // default --seed-intake yes pass files the intake issue as the admin (there
    // is no `human` role). The launcher does NOT suppress intake seeding here.
    assert!(workflow.contains("\"intake_author\": { \"kind\": \"site_admin\" }"));
    let bootstrap = script
        .split("bootstrap_and_provision() {")
        .nth(1)
        .expect("bootstrap_and_provision function exists");
    assert!(
        !bootstrap.contains("--seed-intake no"),
        "intake must be seeded during provisioning, not suppressed"
    );
    assert!(bootstrap.contains("site-admin intake issue"));
    assert!(script.contains("intake_author = site_admin"));
}

#[test]
fn launcher_defaults_basic_forgejo_to_distinct_ports() {
    let script = read_example("run.sh");
    let config = read_example("config/temper.env");

    // Distinct from reference-delivery (4000 / 38080) so both demos coexist.
    assert!(config.contains("BASE_URL=http://127.0.0.1:4100"));
    assert!(config.contains("TRIGGER_BIND=127.0.0.1:38090"));
    assert!(script.contains("BASE_URL=${BASE_URL:-http://127.0.0.1:4100}"));
    assert!(script.contains("TRIGGER_BIND=${TRIGGER_BIND:-127.0.0.1:38090}"));
    assert!(script.contains("*)   PORT=4100 ;;"));
}

#[test]
fn validators_and_config_cover_forgejo_ci_fallback() {
    let script = read_example("run.sh");
    let config = read_example("config/temper.env");

    assert!(config.contains("CI_STATUS_POLL_MS=1000"));
    assert!(config.contains("IDLE_POLL_MAX_MS=8000"));
    assert!(config.contains("FAKE_CI_SENTINEL=present"));
    assert!(script.contains("CI_FALLBACK_MISSING_CREDENTIALS="));
    assert!(script.contains("validate_mechanical_bot_config || _ok=1"));
    assert!(script.contains("validate_mechanical_ci_log || _ok=1"));
    assert!(script.contains("completed tick .*actions="));
    assert!(script.contains("no web-UI credentials configured for the CI read fallback"));
}

#[test]
fn config_workflow_matches_canonical_fixture_byte_for_byte() {
    // config/workflow.json must stay identical to the canonical fixture the
    // temper-workflow tests validate, so the example and the fixture-shape tests
    // never describe two different workflows.
    let example = read_example("config/workflow.json");
    const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/basic-delivery.json");
    assert_eq!(
        example, FIXTURE,
        "examples/basic-delivery/config/workflow.json must match \
         crates/temper-workflow/fixtures/basic-delivery.json byte-for-byte; \
         copy the fixture over the example config when the fixture changes"
    );
}
