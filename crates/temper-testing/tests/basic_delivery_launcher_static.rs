//! Static (source-level) assertions for the `examples/basic-delivery` launcher.
//!
//! These tests pin the launcher to the long-term local-dev UX: jig-backed fake
//! LLM, `temper init --non-interactive`, explicit repo population, and
//! `temper serve standalone` before the seed-last intake issue is filed.

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

fn section<'a>(script: &'a str, marker: &str) -> &'a str {
    script
        .split(marker)
        .nth(1)
        .unwrap_or_else(|| panic!("{marker} section exists"))
}

fn assert_order(haystack: &str, earlier: &str, later: &str) {
    let earlier_index = haystack
        .find(earlier)
        .unwrap_or_else(|| panic!("{earlier:?} exists"));
    let later_index = haystack
        .find(later)
        .unwrap_or_else(|| panic!("{later:?} exists"));
    assert!(
        earlier_index < later_index,
        "expected {earlier:?} before {later:?}"
    );
}

#[test]
fn launcher_uses_init_not_legacy_provisioner() {
    let script = read_example("run.sh");

    assert!(
        !script.contains("provision-forgejo"),
        "happy-path launcher must not call the legacy provisioner"
    );
    assert!(!script.contains("TEMPER_RUN_AUTH"));
    assert!(!script.contains("temper run"));

    let init = section(&script, "run_temper_init() {");
    assert!(init.contains("TEMPER_INIT_ADMIN_PASSWORD=\"$ADMIN_PASSWORD\""));
    assert!(init.contains("TEMPER_INIT_PROVIDER_KEY=\"$INIT_PROVIDER_KEY\""));
    assert!(init.contains("\"$RUN_BIN\" init --non-interactive --force"));
    assert!(init.contains("--forge \"$BASE_URL\""));
    assert!(init.contains("--repo \"$REPO\""));
    assert!(init.contains("--bind \"$DAEMON_BIND\""));
    assert!(init.contains("--admin-user \"$ADMIN_USER\""));
    assert!(init.contains("--provider deepseek"));
    assert!(init.contains("--provider-url \"$JIG_PROVIDER_URL\""));
    assert!(init.contains("--config \"$CONFIG_FILE\""));
    assert!(init.contains("--secrets \"$CREDENTIALS_FILE\""));
}

#[test]
fn launcher_starts_jig_fixture_and_wires_provider_url() {
    let script = read_example("run.sh");
    let config = read_example("config/temper.env");

    assert!(config.contains("JIG_REPO=$HOME/src/rust/jig"));
    assert!(config.contains("JIG_FIXTURE=fixtures/basic-delivery.json"));
    assert!(config.contains("INIT_PROVIDER_KEY=basic-delivery-jig-dummy-key"));

    assert!(script.contains("JIG_REPO=${JIG_REPO:-$HOME/src/rust/jig}"));
    assert!(script.contains("JIG_FIXTURE=${JIG_FIXTURE:-fixtures/basic-delivery.json}"));
    assert!(script.contains("JIG_FIXTURE_PATH=\"$JIG_REPO/$JIG_FIXTURE\""));

    let boot_jig = section(&script, "boot_jig() {");
    assert!(boot_jig.contains("\"$JIG_BIN\" \"$JIG_FIXTURE_PATH\""));
    assert!(boot_jig.contains("JIG_URL=$(sed -n"));
    assert!(boot_jig.contains("JIG_PROVIDER_URL=$JIG_URL"));
    assert!(boot_jig.contains("do not add /v1"));

    let cmd_start = section(&script, "cmd_start() {");
    assert_order(cmd_start, "boot_jig", "run_temper_init");
}

#[test]
fn launcher_uses_init_emitted_artifacts_for_serve_standalone() {
    let script = read_example("run.sh");

    assert!(script.contains("CONFIG_FILE=\"$RUN_DIR/config.toml\""));
    assert!(script.contains("CREDENTIALS_FILE=\"$RUN_DIR/credentials.toml\""));
    assert!(script.contains("INIT_WORKFLOW_PATH=\"$RUN_DIR/workflow.json\""));
    assert!(script.contains("WEBHOOK_SECRET_FILE=\"$RUN_DIR/webhook-secret\""));

    let init = section(&script, "run_temper_init() {");
    assert!(init.contains("temper init did not write $CONFIG_FILE"));
    assert!(init.contains("temper init did not write $CREDENTIALS_FILE"));
    assert!(init.contains("temper init did not write $INIT_WORKFLOW_PATH"));
    assert!(init.contains("temper init did not write $WEBHOOK_SECRET_FILE"));

    let boot_run = section(&script, "boot_run() {");
    assert!(boot_run.contains(
        "\"$RUN_BIN\" serve standalone --config \"$CONFIG_FILE\" --credentials \"$CREDENTIALS_FILE\""
    ));
    assert!(boot_run.contains("webhook listener up"));
    assert!(boot_run.contains("worker: registered"));
    assert!(boot_run.contains("ready -- watching"));
    assert!(!boot_run.contains("daemon --config"));
}

#[test]
fn launcher_populates_repo_before_serve() {
    let script = read_example("run.sh");
    let ci = read_example("config/ci.yml");

    let populate = section(&script, "populate_repo() {");
    assert!(
        populate.contains("cp \"$CONFIG_DIR/ci.yml\" \"$_checkout/.forgejo/workflows/ci.yml\"")
    );
    assert!(populate.contains("cat >\"$_checkout/README.md\""));
    assert!(populate.contains("git -C \"$_checkout\" commit"));
    assert!(populate.contains("git -C \"$_checkout\" push"));
    assert!(populate.contains("files=README.md,.forgejo/workflows/ci.yml"));
    assert!(ci.contains("run.sh commits this file explicitly"));

    let cmd_start = section(&script, "cmd_start() {");
    assert_order(
        cmd_start,
        "\n    run_temper_init\n",
        "\n    populate_repo\n",
    );
    assert_order(cmd_start, "\n    populate_repo\n", "\n    boot_run\n");
}

#[test]
fn launcher_files_intake_after_serve_readiness() {
    let script = read_example("run.sh");
    let workflow = read_example("config/workflow.json");

    assert!(workflow.contains("\"intake_author\": { \"kind\": \"site_admin\" }"));

    let seed_intake = section(&script, "seed_intake() {");
    assert!(seed_intake.contains("TEMPER_FORGEJO_ADMIN_TOKEN=\"$ADMIN_TOKEN\""));
    assert!(seed_intake.contains("/api/v1/repos/{owner_path}/{repo_path}/issues"));
    assert!(seed_intake.contains("\"title\": os.environ[\"TEMPER_INTAKE_TITLE\"]"));
    assert!(seed_intake.contains("\"body\": body"));
    assert!(seed_intake.contains("method=\"POST\""));
    assert!(seed_intake.contains("intake_issue_number=%s intake_issue_url=%s"));
    assert!(!seed_intake.contains("--seed-only"));
    assert!(!seed_intake.contains("--intake-title"));

    let boot_run = section(&script, "boot_run() {");
    assert_order(boot_run, "webhook listener up", "ready -- watching");

    let cmd_start = section(&script, "cmd_start() {");
    assert_order(cmd_start, "boot_run", "seed_intake");
}

#[test]
fn launcher_compatibility_checks_do_not_assume_help_flag_order() {
    let script = read_example("run.sh");

    assert!(script.contains("*--non-interactive*)"));
    assert!(script.contains("*--provider-url*)"));
    assert!(script.contains("*--config*)"));
    assert!(script.contains("*--credentials*)"));
    assert!(
        !script.contains("*--non-interactive*--provider-url*"),
        "the init help check must not depend on clap's flag display order"
    );
    assert!(
        !script.contains("*--config*--credentials*"),
        "the serve help check must not depend on clap's flag display order"
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
    assert!(script.contains("WEBHOOK_URL=http://$DAEMON_BIND/forgejo/webhook"));
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
fn validators_and_config_cover_webhook_and_ci_fallback() {
    let script = read_example("run.sh");

    assert!(script.contains("CI_FALLBACK_MISSING_CREDENTIALS="));
    assert!(script.contains("validate_mechanical_bot_config || _ok=1"));
    assert!(script.contains("validate_mechanical_ci_log || _ok=1"));
    assert!(script.contains("no web-UI credentials configured for the CI read fallback"));

    // Webhooks are the wake path: the validator inspects the unified run log for
    // standalone readiness, accepted deliveries, wake scans, and worker lifecycle.
    assert!(script.contains("ready -- watching"));
    assert!(script.contains("webhook listener up"));
    assert!(script.contains("worker: registered"));
    assert!(script.contains("webhook wake scan"));
}

#[test]
fn config_workflow_matches_canonical_fixture_byte_for_byte() {
    // config/workflow.json must stay identical to the canonical fixture the
    // temper-workflow tests validate, so the example and the fixture-shape tests
    // never describe two different workflows. run.sh uses the workflow.json that
    // `temper init` emits at runtime; this checked-in copy remains the operator
    // reference for that embedded basic-delivery shape.
    let example = read_example("config/workflow.json");
    const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/basic-delivery.json");
    assert_eq!(
        example, FIXTURE,
        "examples/basic-delivery/config/workflow.json must match \
         crates/temper-workflow/fixtures/basic-delivery.json byte-for-byte; \
         copy the fixture over the example config when the fixture changes"
    );
}
