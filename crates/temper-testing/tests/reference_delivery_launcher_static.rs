//! Static (source-level) assertions for the `examples/reference-delivery`
//! launcher.
//!
//! The default reference demo should exercise the fixed cross-repo fan-out path:
//! one source intake fans out into child implementation issues in two repos. The
//! same launcher keeps a separate reviewer-gated single-repo standalone mode.

use std::{fs, path::PathBuf};

fn example_path(relative: &str) -> PathBuf {
    workspace_root()
        .join("examples/reference-delivery")
        .join(relative)
}

fn workspace_root() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::current_dir() {
        candidates.push(path);
    }
    if let Some(value) = std::env::var_os("CARGO_MANIFEST_DIR")
        && !value.is_empty()
    {
        candidates.push(PathBuf::from(value));
    }
    candidates
        .into_iter()
        .find_map(|start| {
            start
                .ancestors()
                .find(|dir| {
                    dir.join("Cargo.toml").is_file()
                        && dir.join("examples/reference-delivery").is_dir()
                })
                .map(PathBuf::from)
        })
        .expect("workspace root resolves")
}

fn read_example(relative: &str) -> String {
    fs::read_to_string(example_path(relative)).expect("example file is readable")
}

#[test]
fn launcher_defaults_to_cross_repo_fanout() {
    let script = read_example("run.sh");
    let cmd_start = script
        .split("cmd_start() {")
        .nth(1)
        .expect("cmd_start function exists")
        .split("cmd_single_repo() {")
        .next()
        .expect("cmd_start body is delimited by cmd_single_repo");

    assert!(cmd_start.contains("cmd_multi_repo"));
    assert!(script.contains("start (default)      run the cross-repo fan-out demo"));
    assert!(script.contains("multi-repo) cmd_multi_repo"));
    assert!(script.contains("single-repo) cmd_single_repo"));
    assert!(script.contains("validate) cmd_validate"));
}

#[test]
fn launcher_keeps_init_apply_and_serve_standalone_for_single_repo_mode() {
    let script = read_example("run.sh");
    let cmd_single = script
        .split("cmd_single_repo() {")
        .nth(1)
        .expect("cmd_single_repo function exists");

    assert!(script.contains("\"$RUN_BIN\" --config \"$CONFIG_FILE\" --secrets \"$CREDENTIALS_FILE\" \\\n                init --non-interactive --force --apply --yes"));
    assert!(script.contains("--workflow \"$WORKFLOW_PATH\""));
    assert!(script.contains(
        "\"$RUN_BIN\" --config \"$CONFIG_FILE\" --secrets \"$CREDENTIALS_FILE\" serve standalone"
    ));

    assert!(!cmd_single.contains("provision-forgejo"));
    assert!(!cmd_single.contains("validate-reference-delivery"));
    assert!(!script.contains("\"$RUN_BIN\" daemon"));
    assert!(!script.contains("temper run"));
    assert!(!script.contains("FORGEJO_ACCESS_TOKEN"));
    assert!(!script.contains("TEMPER_FORGEJO_TOKEN_"));
}

#[test]
fn launcher_is_fixed_and_has_no_operator_env_config_file() {
    let script = read_example("run.sh");

    assert!(script.contains("BASE_URL=http://127.0.0.1:4200"));
    assert!(script.contains("DAEMON_BIND=127.0.0.1:38200"));
    assert!(script.contains("REPO=$OWNER/$NAME"));
    assert!(script.contains("JIG_FIXTURE_PATH=\"$JIG_REPO/fixtures/reference-delivery.json\""));
    assert!(
        script.contains("The default reference-delivery demo intentionally provisions exactly")
    );
    assert!(script.contains("MULTI_REPOS=\"$REPO $CANARY_REPO\""));

    assert!(!example_path("config/temper.env").exists());
    assert!(!script.contains("TEMPER_REFERENCE_DELIVERY_SCRIPT_DIR"));
    assert!(!script.contains("TEMPER_RUN_BIN"));
    assert!(!script.contains("SERVED_ROLES="));
}

#[test]
fn launcher_exposes_fixed_multi_repo_fanout_mode() {
    let script = read_example("run.sh");

    assert!(script.contains("multi-repo) cmd_multi_repo"));
    assert!(script.contains("CANARY_NAME=service-canary"));
    assert!(script.contains("CANARY_REPO=$OWNER/$CANARY_NAME"));
    assert!(script.contains("cargo build -p temper-testing --bin temper-testing-worker"));
    assert!(script.contains("\"$RUN_BIN\" provision-forgejo"));
    assert!(script.contains("\"$WORKER_BIN\""));
    assert!(script.contains("--repo \"$REPO\""));
    assert!(script.contains("--repo \"$CANARY_REPO\""));
    assert!(script.contains("--architect closing"));
    assert!(script.contains("validate-reference-delivery"));
    assert!(script.contains("--expected-children 2"));
    assert!(script.contains("\"target_repo\": \"forgejo:$CANARY_REPO\""));
    assert!(script.contains("validate-multi-repo) cmd_validate_multi_repo"));
    assert!(script.contains("field == \"user\""));
    assert!(script.contains("value = role"));
}

#[test]
fn launcher_preflights_fixed_binds_and_stale_pids() {
    let script = read_example("run.sh");
    let cmd_multi = script
        .split("cmd_multi_repo() {")
        .nth(1)
        .expect("cmd_multi_repo function exists")
        .split("cmd_start() {")
        .next()
        .expect("cmd_multi_repo body is delimited by cmd_start");
    let cmd_single = script
        .split("cmd_single_repo() {")
        .nth(1)
        .expect("cmd_single_repo function exists");

    assert!(script.contains("assert_no_active_run()"));
    assert!(script.contains("assert_bind_available()"));
    assert!(cmd_multi.contains("assert_no_active_run"));
    assert!(cmd_multi.contains("assert_bind_available 'Forgejo' \"$HOST:$PORT\""));
    assert!(cmd_single.contains("assert_no_active_run"));
    assert!(cmd_single.contains("assert_bind_available 'Forgejo' \"$HOST:$PORT\""));
    assert!(
        cmd_single.contains("assert_bind_available 'temper serve standalone' \"$DAEMON_BIND\"")
    );
}

#[test]
fn launcher_keeps_runtime_credentials_in_init_bundle() {
    let script = read_example("run.sh");

    assert!(script.contains("CONFIG_FILE=\"$RUN_DIR/config.toml\""));
    assert!(script.contains("CREDENTIALS_FILE=\"$RUN_DIR/credentials.toml\""));
    assert!(script.contains("WEBHOOK_SECRET_FILE=\"$RUN_DIR/webhook-secret\""));
    assert!(
        script.contains("\"$RUN_BIN\" --config \"$CONFIG_FILE\" --secrets \"$CREDENTIALS_FILE\"")
    );
    assert!(script.contains("TEMPER_INIT_ADMIN_PASSWORD=\"$ADMIN_PASSWORD\""));
    assert!(script.contains("TEMPER_INIT_PROVIDER_KEY=\"$INIT_PROVIDER_KEY\""));

    assert!(!script.contains("secrets/credentials.toml"));
    assert!(!script.contains("roles.env"));
}

#[test]
fn launcher_serves_reviewer_but_not_owner_or_human() {
    let script = read_example("run.sh");
    let limit = script
        .split("limit_served_roles() {")
        .nth(1)
        .expect("limit_served_roles function exists");

    assert!(limit.contains("roles = [\"architect\", \"engineer\", \"reviewer\"]"));
    assert!(script.contains("served_roles=architect,engineer,reviewer"));
    assert!(script.contains("role=\"reviewer\""));
}

#[test]
fn launcher_seeds_intake_after_standalone_readiness_in_single_repo_mode() {
    let script = read_example("run.sh");
    let cmd_start = script
        .split("cmd_single_repo() {")
        .nth(1)
        .expect("cmd_single_repo function exists");

    let jig_index = cmd_start
        .find("\n    boot_jig")
        .expect("boot_jig is called");
    let init_index = cmd_start
        .find("\n    run_temper_init")
        .expect("run_temper_init is called");
    let boot_index = cmd_start
        .find("\n    boot_run\n")
        .expect("boot_run is called");
    let seed_index = cmd_start
        .find("\n    seed_intake")
        .expect("seed_intake is called");
    assert!(
        jig_index < init_index,
        "jig URL feeds temper init provider-url"
    );
    assert!(
        init_index < boot_index,
        "init must write config before serve"
    );
    assert!(
        boot_index < seed_index,
        "intake must be seeded after readiness"
    );

    let boot_run = script
        .split("boot_run() {")
        .nth(1)
        .expect("boot_run function exists");
    assert!(boot_run.contains("webhook listener up"));
    assert!(boot_run.contains("worker:  capacity:"));
    assert!(boot_run.contains("ready -- watching"));
}

#[test]
fn validator_checks_reviewer_gated_landing_evidence() {
    let script = read_example("run.sh");

    assert!(script.contains("event=\"wake.received\""));
    assert!(script.contains("mark_untriaged applied"));
    assert!(script.contains("role=\"architect\""));
    assert!(script.contains("role=\"engineer\""));
    assert!(script.contains("role=\"reviewer\""));
    assert!(script.contains("event=\"pr.merged\""));
    assert!(script.contains("event=\"item.resolved\""));
    assert!(script.contains("no web-UI credentials configured for the CI read fallback"));
}
