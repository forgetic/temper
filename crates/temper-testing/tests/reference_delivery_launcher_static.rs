use std::{fs, path::PathBuf};

fn example_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("examples/reference-delivery")
        .join(relative)
}

fn read_example(relative: &str) -> String {
    fs::read_to_string(example_path(relative)).expect("example file is readable")
}

#[test]
fn mechanical_worker_gets_ci_web_ui_credentials_without_argv_secrets() {
    let script = read_example("run.sh");
    let launch_workers = script
        .split("launch_workers() {")
        .nth(1)
        .expect("launch_workers function exists");
    let mechanical_spawn = launch_workers
        .split("TEMPER_FORGEJO_TOKEN=\"$ADMIN_TOKEN\"")
        .nth(1)
        .expect("mechanical worker uses the admin REST token")
        .split(") >\"$LOG_DIR/mechanical.log\"")
        .next()
        .expect("mechanical spawn stanza is bounded");

    assert!(mechanical_spawn.contains("TEMPER_FORGEJO_USERNAME=\"$ENGINEER_USER\""));
    assert!(launch_workers.contains("ci_reader_role=engineer"));
    assert!(mechanical_spawn.contains("TEMPER_FORGEJO_PASSWORD=\"$ENGINEER_PASSWORD\""));
    assert!(mechanical_spawn.contains("--poll-ms \"$CI_STATUS_POLL_MS\""));

    let argv = mechanical_spawn
        .split("\"$TESTING_WORKER_BIN\"")
        .nth(1)
        .expect("worker argv follows the binary path");
    assert!(!argv.contains("ENGINEER_PASSWORD"));
    assert!(!argv.contains("ADMIN_TOKEN"));
    assert!(!argv.contains("TEMPER_FORGEJO_PASSWORD"));
    assert!(!argv.contains("TEMPER_FORGEJO_TOKEN"));
    assert!(!argv.contains("--password"));
    assert!(!argv.contains("--token"));
}

#[test]
fn mechanical_ci_reader_is_resolved_before_worker_launch() {
    let script = read_example("run.sh");
    assert!(script.contains("mechanical CI reader role 'engineer' has no username"));
    assert!(script.contains("mechanical CI reader role 'engineer' has no password"));

    let launch_workers = script
        .split("launch_workers() {")
        .nth(1)
        .expect("launch_workers function exists");
    let resolve_index = launch_workers
        .find("resolve_mechanical_ci_reader")
        .expect("mechanical CI reader is resolved");
    let spawn_index = launch_workers
        .find("TEMPER_FORGEJO_TOKEN=\"$ADMIN_TOKEN\"")
        .expect("mechanical worker spawn exists");
    assert!(
        resolve_index < spawn_index,
        "mechanical CI credentials should fail before the worker process starts"
    );
}

#[test]
fn validators_and_config_cover_forgejo_ci_fallback() {
    let script = read_example("run.sh");
    let config = read_example("config/temper.env");

    assert!(config.contains("CI_STATUS_POLL_MS=1000"));
    assert!(script.contains("CI_FALLBACK_MISSING_CREDENTIALS="));
    assert!(script.contains("validate_mechanical_ci_reader_config || _ok=1"));
    assert!(script.contains("validate_mechanical_ci_log || _ok=1"));
    assert!(script.contains("completed tick .*actions="));
    assert!(script.contains("no web-UI credentials configured for the CI read fallback"));
}
