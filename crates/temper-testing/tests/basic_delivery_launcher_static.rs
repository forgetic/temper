//! Static (source-level) assertions for the `examples/basic-delivery` launcher.
//!
//! Mirrors `reference_delivery_launcher_static.rs` at basic-delivery scope: it
//! reads `run.sh` / `config/temper.env` as text and asserts the invariants that
//! keep the no-human-in-the-loop demo correct and secret-safe — `temper run` gets
//! the `bot` credentials via the environment (never argv), the bundled 3-role
//! workflow reaches both the provision and the run invocations, intake is seeded
//! last as the site admin (the seed-last webhook-wake proof), the ports are
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
                    dir.join("Cargo.toml").is_file() && dir.join("examples/basic-delivery").is_dir()
                })
                .map(PathBuf::from)
        })
        .expect("workspace root resolves")
}

fn read_example(relative: &str) -> String {
    fs::read_to_string(example_path(relative)).expect("example file is readable")
}

/// The `temper run` process gets the bot automation credentials via the
/// environment, never on argv: the mechanical landing path and the ADR-0019 CI
/// read fallback both need them, but secrets must not appear on a command line.
#[test]
fn temper_run_gets_bot_credentials_without_argv_secrets() {
    let script = read_example("run.sh");
    let boot_run = script
        .split("boot_run() {")
        .nth(1)
        .expect("boot_run function exists");
    let spawn = boot_run
        .split("FORGEJO_ACCESS_TOKEN=\"$BOT_TOKEN\"")
        .nth(1)
        .expect("temper run uses the bot REST token")
        .split(") >\"$LOG_DIR/run.log\"")
        .next()
        .expect("temper run spawn stanza is bounded");

    assert!(spawn.contains("FORGEJO_USERNAME=\"$BOT_USER\""));
    assert!(spawn.contains("FORGEJO_PASSWORD=\"$BOT_PASSWORD\""));

    // Neither the argv nor the generated config file carries a secret value.
    assert!(!boot_run.contains("--password"));
    assert!(!boot_run.contains("--token"));
    assert!(!boot_run.contains("--bot-token"));
    // The run is launched config-driven (`temper daemon --config`); the only
    // secret-bearing env assignments are the FORGEJO_* lines above.
    assert!(boot_run.contains("daemon --config \"$_config\""));
}

/// The provisioner now writes the runtime's own `credentials.toml` (no env file),
/// and the daemon loads the per-role + bot identities from it via
/// `--credentials`. No legacy `roles.env` / per-role env exports remain.
#[test]
fn launcher_is_credentials_toml_driven() {
    let script = read_example("run.sh");

    // The provisioner writes credentials.toml, never a roles.env env file.
    assert!(script.contains("CREDENTIALS_FILE=\"$SECRETS_DIR/credentials.toml\""));
    assert!(!script.contains("roles.env"));
    assert!(script.contains("--out \"$CREDENTIALS_FILE\""));

    // The daemon reads role/bot identities from credentials.toml via --credentials.
    let boot_run = script
        .split("boot_run() {")
        .nth(1)
        .expect("boot_run function exists");
    assert!(boot_run.contains("--credentials \"$CREDENTIALS_FILE\""));

    // The legacy per-role env exports are gone; identities come from the file.
    assert!(!script.contains("export_run_role_env"));
    assert!(!script.contains("TEMPER_FORGEJO_USER_"));
    assert!(!script.contains("TEMPER_FORGEJO_TOKEN_"));
}

#[test]
fn bot_identity_is_resolved_before_temper_run_launch() {
    let script = read_example("run.sh");
    assert!(script.contains("automation user 'bot' has no username"));
    assert!(script.contains("automation user must be 'bot'"));
    assert!(script.contains("automation user 'bot' has no token"));
    assert!(script.contains("automation user 'bot' has no password"));

    let boot_run = script
        .split("boot_run() {")
        .nth(1)
        .expect("boot_run function exists");
    let resolve_index = boot_run
        .find("resolve_bot_identity")
        .expect("bot identity is resolved");
    let spawn_index = boot_run
        .find("FORGEJO_ACCESS_TOKEN=\"$BOT_TOKEN\"")
        .expect("temper run spawn exists");
    assert!(
        resolve_index < spawn_index,
        "bot credentials should fail before the temper run process starts"
    );
}

#[test]
fn launcher_uses_non_reserved_setup_admin_handle() {
    let script = read_example("run.sh");

    // Forgejo reserves the literal username `admin`; the throwaway setup admin
    // uses a valid non-reserved handle.
    assert!(script.contains("ADMIN_USER=basicadmin"));
    assert!(!script.contains("ADMIN_USER=admin\n"));
}

#[test]
fn launcher_passes_workflow_to_provision_and_run() {
    let script = read_example("run.sh");
    let config = read_example("config/temper.env");

    // The bundled 3-role spec reaches the provision invocation via
    // --workflow "$WORKFLOW_PATH" and the run via the config file's
    // workflow = "$WORKFLOW_PATH".
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

    let boot_run = script
        .split("boot_run() {")
        .nth(1)
        .expect("boot_run function exists");
    // The config-driven run writes the workflow + bind into the generated config
    // and launches the standalone daemon.
    assert!(
        boot_run.contains("workflow = \"$WORKFLOW_PATH\""),
        "config must set the workflow path"
    );
    assert!(boot_run.contains("bind = \"$DAEMON_BIND\""));
    assert!(boot_run.contains("daemon --config"));
    // The run hosts the real in-process coding agent: it selects an LLM provider
    // (derived from TEMPER_RUN_AUTH), not a fake-agent profile.
    assert!(boot_run.contains("TEMPER_RUN_AUTH"));
    assert!(boot_run.contains("provider = \"$_provider\""));
}

#[test]
fn launcher_files_intake_last_as_site_admin_via_forgejo_api() {
    let script = read_example("run.sh");
    let workflow = read_example("config/workflow.json");

    // The bundled spec declares intake_author = site_admin, so the direct API
    // call files the intake issue as the admin (there is no `human` role).
    assert!(workflow.contains("\"intake_author\": { \"kind\": \"site_admin\" }"));

    // Seed-last: the provision pass holds the intake back (--seed-intake no), then
    // a direct Forgejo REST API issue create files it AFTER temper run is ready,
    // so the issue-created webhook is the demonstrated wake path.
    let bootstrap = script
        .split("bootstrap_and_provision() {")
        .nth(1)
        .expect("bootstrap_and_provision function exists");
    assert!(
        bootstrap.contains("--seed-intake no"),
        "the provision pass must hold the intake back for the seed-last proof"
    );
    let seed_intake = script
        .split("seed_intake() {")
        .nth(1)
        .expect("seed_intake function exists");
    assert!(seed_intake.contains("TEMPER_FORGEJO_ADMIN_TOKEN=\"$ADMIN_TOKEN\""));
    assert!(seed_intake.contains("/api/v1/repos/{owner_path}/{repo_path}/issues"));
    assert!(seed_intake.contains("\"title\": os.environ[\"TEMPER_INTAKE_TITLE\"]"));
    assert!(seed_intake.contains("\"body\": body"));
    assert!(seed_intake.contains("method=\"POST\""));
    assert!(seed_intake.contains("intake_issue_number=%s intake_issue_url=%s"));
    assert!(!seed_intake.contains("--seed-only"));
    assert!(!seed_intake.contains("--intake-title"));

    // seed_intake runs after boot_run in cmd_start.
    let cmd_start = script
        .split("cmd_start() {")
        .nth(1)
        .expect("cmd_start function exists");
    let boot_index = cmd_start.find("boot_run").expect("boot_run is called");
    let seed_index = cmd_start
        .find("seed_intake")
        .expect("seed_intake is called");
    assert!(
        boot_index < seed_index,
        "intake must be seeded only after temper run is ready"
    );
}

#[test]
fn launcher_defaults_basic_forgejo_to_distinct_ports() {
    let script = read_example("run.sh");
    let config = read_example("config/temper.env");

    // Distinct from reference-delivery (4200 / 38200) so both demos coexist.
    assert!(config.contains("BASE_URL=http://127.0.0.1:4100"));
    assert!(config.contains("DAEMON_BIND=127.0.0.1:38100"));
    assert!(script.contains("BASE_URL=${BASE_URL:-http://127.0.0.1:4100}"));
    assert!(script.contains("DAEMON_BIND=${DAEMON_BIND:-127.0.0.1:38100}"));
}

#[test]
fn validators_and_config_cover_forgejo_ci_fallback() {
    let script = read_example("run.sh");

    assert!(script.contains("CI_FALLBACK_MISSING_CREDENTIALS="));
    assert!(script.contains("validate_mechanical_bot_config || _ok=1"));
    assert!(script.contains("validate_mechanical_ci_log || _ok=1"));
    assert!(script.contains("no web-UI credentials configured for the CI read fallback"));

    // Webhooks are the wake path: the validator inspects the unified run log for
    // the daemon serving line, accepted deliveries, wake scans, and the
    // in-process worker lifecycle.
    assert!(script.contains("engine:  serving on"));
    assert!(script.contains("worker: registered"));
    assert!(script.contains("webhook wake scan"));
}

#[test]
fn launcher_runs_a_single_temper_run_process() {
    let script = read_example("run.sh");

    // The migration collapsed the split daemon + worker (+ trigger) into ONE
    // process. The legacy fake-worker / trigger machinery must be gone.
    assert!(!script.contains("temper-testing-worker"));
    assert!(!script.contains("smith-worker"));
    assert!(!script.contains("temper-trigger-forgejo"));
    assert!(!script.contains("--wake-socket"));
    assert!(!script.contains("anvil-agent"));
    assert!(!script.contains("--agent-command"));

    // The unified entry point is the config-driven `temper daemon`.
    assert!(script.contains("\"$RUN_BIN\" daemon --config"));
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
