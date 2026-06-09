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

    assert!(script.contains("ADMIN_USER=siteadmin"));
    assert!(!script.contains("ADMIN_USER=admin\n"));
    assert!(script.contains("Forgejo reserves the"));
}

#[test]
fn launcher_defaults_reference_forgejo_to_port_4000() {
    let script = read_example("run.sh");
    let config = read_example("config/temper.env");

    assert!(config.contains("BASE_URL=http://127.0.0.1:4000"));
    assert!(script.contains("BASE_URL=${BASE_URL:-http://127.0.0.1:4000}"));
    assert!(script.contains("*)   PORT=4000 ;;"));
}

#[test]
fn validators_and_config_cover_forgejo_ci_fallback() {
    let script = read_example("run.sh");
    let config = read_example("config/temper.env");

    assert!(config.contains("CI_STATUS_POLL_MS=30000"));
    assert!(config.contains("IDLE_POLL_MAX_MS=8000"));
    assert!(script.contains("CI_FALLBACK_MISSING_CREDENTIALS="));
    assert!(script.contains("validate_mechanical_bot_config || _ok=1"));
    assert!(script.contains("validate_mechanical_ci_log || _ok=1"));
    assert!(script.contains("completed tick .*actions="));
    assert!(script.contains("no web-UI credentials configured for the CI read fallback"));
}
