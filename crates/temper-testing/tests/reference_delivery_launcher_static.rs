//! Static (source-level) assertions for the `examples/reference-delivery`
//! launcher.
//!
//! Reads `run.sh` / `config/temper.env` as text and asserts the invariants that
//! keep the demo correct and secret-safe under the unified `temper run` topology:
//! `temper run` gets the `bot` credentials via the environment (never argv), the
//! bundled workflow reaches both the provision and the run invocations, intake is
//! seeded last (the seed-last webhook-wake proof), the reviewer role is served,
//! the ports are distinct from basic-delivery, and the ADR-0019 CI read fallback
//! knobs are present.

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

/// The `temper run` process gets the bot automation credentials via the
/// environment, never on argv.
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

    assert!(!boot_run.contains("--password"));
    assert!(!boot_run.contains("--token"));
    assert!(!boot_run.contains("--bot-token"));
    assert!(boot_run.contains("daemon --config \"$_config\""));
}

/// The provisioner now writes the runtime's own `credentials.toml` (no env file),
/// and the daemon loads the per-role + bot identities from it via
/// `--secrets`. No legacy `roles.env` / per-role env exports remain.
#[test]
fn launcher_is_credentials_toml_driven() {
    let script = read_example("run.sh");

    assert!(script.contains("CREDENTIALS_FILE=\"$SECRETS_DIR/credentials.toml\""));
    assert!(!script.contains("roles.env"));
    assert!(script.contains("--out \"$CREDENTIALS_FILE\""));

    let boot_run = script
        .split("boot_run() {")
        .nth(1)
        .expect("boot_run function exists");
    assert!(boot_run.contains("--secrets \"$CREDENTIALS_FILE\""));
    assert!(boot_run.contains("admin = \"bot\""));
    assert!(boot_run.contains("ci_user = \"bot\""));

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

    assert!(script.contains("ADMIN_USER=refadmin"));
    assert!(!script.contains("ADMIN_USER=admin\n"));
}

#[test]
fn launcher_defaults_reference_forgejo_to_distinct_ports() {
    let script = read_example("run.sh");
    let config = read_example("config/temper.env");

    // Distinct from basic-delivery (4100 / 38100) so both demos coexist.
    assert!(config.contains("BASE_URL=http://127.0.0.1:4200"));
    assert!(config.contains("DAEMON_BIND=127.0.0.1:38200"));
    assert!(script.contains("BASE_URL=${BASE_URL:-http://127.0.0.1:4200}"));
    assert!(script.contains("DAEMON_BIND=${DAEMON_BIND:-127.0.0.1:38200}"));
}

#[test]
fn launcher_passes_workflow_and_serves_reviewer() {
    let script = read_example("run.sh");
    let config = read_example("config/temper.env");

    assert!(config.contains("WORKFLOW_FILE=workflow.json"));
    // reference-delivery adds the reviewer gate on top of architect + engineer.
    assert!(config.contains("SERVED_ROLES=\"architect engineer reviewer\""));

    let bootstrap = script
        .split("bootstrap_and_provision() {")
        .nth(1)
        .expect("bootstrap_and_provision function exists");
    assert!(bootstrap.contains("--workflow \"$WORKFLOW_PATH\""));

    let boot_run = script
        .split("boot_run() {")
        .nth(1)
        .expect("boot_run function exists");
    // The config-driven run writes workflow + deterministic jig provider wiring.
    assert!(boot_run.contains("workflow = \"$WORKFLOW_PATH\""));
    assert!(boot_run.contains("_provider=deepseek"));
    assert!(boot_run.contains("provider = \"$_provider\""));
    assert!(boot_run.contains("[agent.providers.deepseek]"));
    assert!(boot_run.contains("url = \"$JIG_URL\""));
    assert!(config.contains("TEMPER_JIG_REPO="));
    assert!(config.contains("TEMPER_JIG_BIN="));
    assert!(config.contains("TEMPER_REFERENCE_DELIVERY_JIG_FIXTURE="));
    assert!(!config.contains("TEMPER_REFERENCE_DELIVERY_JIG_BIN="));
    assert!(!script.contains("TEMPER_RUN_AUTH"));
    // Roles (incl. reviewer) and repos are rendered into the config's TOML arrays.
    assert!(boot_run.contains("$SERVED_ROLES"));
    assert!(boot_run.contains("$CONFIGURED_REPOS"));
}

#[test]
fn launcher_seeds_intake_last() {
    let script = read_example("run.sh");

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
    assert!(seed_intake.contains("--seed-only"));

    let cmd_start = script
        .split("cmd_start() {")
        .nth(1)
        .expect("cmd_start function exists");
    let jig_index = cmd_start
        .find("\n    boot_jig")
        .expect("boot_jig is called");
    let boot_index = cmd_start
        .find("\n    boot_run\n")
        .expect("boot_run is called");
    let seed_index = cmd_start
        .find("\n    seed_intake")
        .expect("seed_intake is called");
    assert!(
        jig_index < boot_index,
        "jig must be ready before temper run starts"
    );
    assert!(
        boot_index < seed_index,
        "intake must be seeded only after temper run is ready"
    );
}

#[test]
fn validators_and_config_cover_forgejo_ci_fallback() {
    let script = read_example("run.sh");

    assert!(script.contains("CI_FALLBACK_MISSING_CREDENTIALS="));
    assert!(script.contains("validate_mechanical_bot_config || _ok=1"));
    assert!(script.contains("validate_mechanical_ci_log || _ok=1"));
    assert!(script.contains("no web-UI credentials configured for the CI read fallback"));

    // Webhooks are the wake path: the validator inspects the unified run log.
    assert!(script.contains("webhook listener up"));
    assert!(script.contains("worker:  capacity:"));
    assert!(script.contains("ready -- watching"));
    assert!(script.contains("event=\"wake.received\""));
    assert!(script.contains("mark_untriaged applied"));

    // The cross-repo Forge-state validator is still wired in, as a subcommand of
    // the unified binary.
    assert!(script.contains("validate-reference-delivery"));
    assert!(script.contains("cmd_validate_multi_repo"));
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

    assert!(!script.contains("temper-reference-delivery-jig"));
    assert!(script.contains("JIG_REPO=${TEMPER_JIG_REPO:-$HOME/src/rust/jig}"));
    assert!(script.contains("JIG_BIN=${TEMPER_JIG_BIN:-$JIG_REPO/target/debug/jig}"));
    assert!(script.contains("fixtures/reference-delivery.json"));
    assert!(script.contains("cargo build -p jig"));
    assert!(script.contains("\"$JIG_BIN\" \"$JIG_FIXTURE_PATH\""));
    assert!(script.contains("JIG_URL=$(sed -n"));
    assert!(script.contains("do not add /v1"));
    assert!(script.contains("\"$RUN_BIN\" daemon --config"));
}
