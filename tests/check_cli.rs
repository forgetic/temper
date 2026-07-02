// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

const WORKFLOW_JSON: &str =
    include_str!("../crates/temper-workflow/fixtures/reference-delivery.json");

fn temper(args: &[&str], env_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper"))
        .args(args)
        .env("XDG_CONFIG_HOME", env_root.join("xdg-config"))
        .env("XDG_STATE_HOME", env_root.join("xdg-state"))
        .env("HOME", env_root.join("home"))
        .output()
        .expect("run temper")
}

fn write_valid_bundle(root: &Path) -> std::path::PathBuf {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [deployment]\n\
         name = \"local-dev\"\n\
         topology = \"standalone\"\n\
         [workflow]\n\
         file = \"workflow.json\"\n\
         [paths]\n\
         state_dir = \"state\"\n\
         workspace_dir = \"workspace\"\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         admin = \"engineer\"\n\
         ci_user = \"engineer\"\n\
         [engine]\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n",
    )
    .expect("write config");
    std::fs::write(bundle.join("workflow.json"), WORKFLOW_JSON).expect("write workflow");
    std::fs::create_dir_all(bundle.join("state")).expect("create state dir");
    std::fs::create_dir_all(bundle.join("workspace")).expect("create workspace dir");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"forge-token\"\n\
         password = \"forge-password\"\n\
         [agent.providers.anthropic]\n\
         type = \"api-key\"\n\
         key = \"provider-key\"\n",
    )
    .expect("write credentials");
    bundle
}

#[test]
fn top_level_help_lists_check_and_hides_internal_agent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["--help"], dir.path());

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("\n  check "), "{stdout}");
    assert!(!stdout.contains("\n  agent "), "{stdout}");
}

#[test]
fn check_help_exits_successfully() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["check", "--help"], dir.path());

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("Usage: temper [GLOBAL OPTIONS] check"),
        "{stdout}"
    );
    assert!(stdout.contains("--component"), "{stdout}");
    assert!(stdout.contains("--pool"), "{stdout}");
    assert!(stdout.contains("--strict"), "{stdout}");
}

#[test]
fn check_rejects_pool_outside_worker_component() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(
        &["check", "--component", "engine", "--pool", "builders"],
        dir.path(),
    );

    assert!(
        !output.status.success(),
        "pool without worker is a usage error"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("--pool is only valid"), "{stderr}");
}

#[test]
fn check_fails_without_config_files_but_prints_report() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["check"], dir.path());

    assert!(
        !output.status.success(),
        "missing config should report blocking validation findings"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("config:      (none"),
        "default environment should not load a config file: {stdout}"
    );
    assert!(
        stdout.contains("credentials: (none"),
        "default environment should not load credentials: {stdout}"
    );
    assert!(stdout.contains("error: forge URL is unset"), "{stdout}");
}

#[test]
fn check_json_reports_status_findings_and_loaded_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["--format", "json", "check"], dir.path());

    assert!(
        !output.status.success(),
        "JSON status=error should also return a non-zero process status"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");

    assert_eq!(value["status"], "error");
    assert_eq!(value["result"], "error");
    assert!(value["config_path"].is_null(), "{value}");
    assert!(value["credentials_path"].is_null(), "{value}");
    assert!(value["paths"]["config"].is_null(), "{value}");
    assert!(value["paths"]["credentials"].is_null(), "{value}");

    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["severity"] == "error"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("forge URL is unset"))),
        "{value}"
    );
    assert!(
        findings.iter().any(|finding| finding["severity"] == "note"),
        "{value}"
    );
}

#[test]
fn check_json_succeeds_for_valid_explicit_bundle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_valid_bundle(dir.path());
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &["--config", &bundle_arg, "--format", "json", "check"],
        dir.path(),
    );

    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");

    assert_eq!(value["status"], "ok");
    assert_eq!(value["result"], "ok");
    assert_eq!(
        value["paths"]["config"],
        bundle.join("config.toml").display().to_string()
    );
    assert_eq!(
        value["paths"]["credentials"],
        bundle.join("credentials.toml").display().to_string()
    );
    assert!(
        value["findings"].as_array().is_some_and(Vec::is_empty),
        "{value}"
    );
}

#[test]
fn check_json_fails_for_target_legacy_workflow_conflict() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [workflow]\n\
         file = \"target-workflow.json\"\n\
         [engine]\n\
         workflow = \"legacy-workflow.json\"\n",
    )
    .expect("write config");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &["--config", &bundle_arg, "--format", "json", "check"],
        dir.path(),
    );

    assert!(!output.status.success(), "conflict should fail check");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["severity"] == "error"
            && finding["message"].as_str().is_some_and(|message| {
                message.contains("workflow.file")
                    && message.contains("engine.workflow")
                    && message.contains("conflicting")
            })),
        "{value}"
    );
}

#[test]
fn check_json_fails_for_target_pool_profile_validation_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [[worker.pools]]\n\
         name = \"engineers\"\n\
         roles = [\"engineer\"]\n\
         max_concurrent_jobs = 0\n",
    )
    .expect("write config");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &["--config", &bundle_arg, "--format", "json", "check"],
        dir.path(),
    );

    assert!(!output.status.success(), "invalid pool should fail check");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["severity"] == "error"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("worker.pools[0].max_concurrent_jobs"))),
        "{value}"
    );
}

#[test]
fn check_json_fails_for_missing_named_secret_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         [engine]\n\
         forge_token = \"missing-engine-token\"\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n",
    )
    .expect("write config");
    std::fs::write(bundle.join("credentials.toml"), "schema_version = 1\n")
        .expect("write credentials");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &["--config", &bundle_arg, "--format", "json", "check"],
        dir.path(),
    );

    assert!(!output.status.success(), "missing secret should fail check");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["severity"] == "error"
            && finding["message"].as_str().is_some_and(|message| {
                message.contains("engine.forge_token") && message.contains("missing-engine-token")
            })),
        "{value}"
    );
}

#[test]
fn check_json_succeeds_for_config_relative_yaml_workflow() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(bundle.join("flows")).expect("create flows");
    std::fs::create_dir_all(bundle.join("state")).expect("create state");
    std::fs::create_dir_all(bundle.join("workspace")).expect("create workspace");
    std::fs::write(bundle.join("flows/workflow.yaml"), WORKFLOW_JSON).expect("write workflow");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [workflow]\n\
         file = \"flows/workflow.yaml\"\n\
         [paths]\n\
         state_dir = \"state\"\n\
         workspace_dir = \"workspace\"\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         admin = \"engineer\"\n\
         ci_user = \"engineer\"\n\
         [engine]\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n",
    )
    .expect("write config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"forge-token\"\n\
         password = \"forge-password\"\n\
         [agent.providers.anthropic]\n\
         type = \"api-key\"\n\
         key = \"provider-key\"\n",
    )
    .expect("write credentials");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &["--config", &bundle_arg, "--format", "json", "check"],
        dir.path(),
    );

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["status"], "ok");
    assert!(
        value["findings"].as_array().is_some_and(Vec::is_empty),
        "{value}"
    );
}

#[test]
fn check_json_fails_for_missing_workflow_file_with_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_valid_bundle(dir.path());
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [workflow]\n\
         file = \"missing.json\"\n\
         [paths]\n\
         state_dir = \"state\"\n\
         workspace_dir = \"workspace\"\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         admin = \"engineer\"\n\
         ci_user = \"engineer\"\n\
         [engine]\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n",
    )
    .expect("rewrite config");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &["--config", &bundle_arg, "--format", "json", "check"],
        dir.path(),
    );

    assert!(!output.status.success(), "missing workflow should fail");
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["message"]
            .as_str()
            .is_some_and(|message| message.contains("failed to read workflow file")
                && message.contains("missing.json"))),
        "{value}"
    );
}

#[test]
fn check_json_fails_for_invalid_yaml_workflow_with_format() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_valid_bundle(dir.path());
    std::fs::write(bundle.join("broken.yaml"), "name: [unterminated\n").expect("write yaml");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [workflow]\n\
         file = \"broken.yaml\"\n\
         [paths]\n\
         state_dir = \"state\"\n\
         workspace_dir = \"workspace\"\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         admin = \"engineer\"\n\
         ci_user = \"engineer\"\n\
         [engine]\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n",
    )
    .expect("rewrite config");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &["--config", &bundle_arg, "--format", "json", "check"],
        dir.path(),
    );

    assert!(!output.status.success(), "invalid YAML should fail");
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(
            |finding| finding["message"].as_str().is_some_and(|message| message
                .contains("not valid YAML")
                && message.contains("broken.yaml"))
        ),
        "{value}"
    );
}

#[test]
fn worker_pool_scope_ignores_unrelated_engine_secret() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(bundle.join("state")).expect("create state");
    std::fs::create_dir_all(bundle.join("workspace")).expect("create workspace");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [paths]\n\
         state_dir = \"state\"\n\
         workspace_dir = \"workspace\"\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         [engine]\n\
         forge_token = \"missing-engine-token\"\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"architect\"]\n\
         [[worker.pools]]\n\
         name = \"engineers\"\n\
         roles = [\"engineer\"]\n\
         repos = [\"ai/temper\"]\n\
         agent_profile = \"coding\"\n\
         [agent.profiles.coding]\n\
         credential = \"profile-secret\"\n",
    )
    .expect("write config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"role-token\"\n\
         [secrets]\n\
         profile-secret = \"provider-key\"\n",
    )
    .expect("write credentials");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--component",
            "worker",
            "--pool",
            "engineers",
        ],
        dir.path(),
    );

    assert!(
        output.status.success(),
        "worker scope should ignore engine-only secret: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["component"], "worker");
    assert_eq!(value["pool"], "engineers");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().all(|finding| !finding["message"]
            .as_str()
            .is_some_and(|message| message.contains("engine.forge_token"))),
        "{value}"
    );
}

#[test]
fn strict_promotes_online_note_to_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_valid_bundle(dir.path());
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--online",
            "--strict",
        ],
        dir.path(),
    );

    assert!(!output.status.success(), "strict note should fail");
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["status"], "error");
    assert!(value["strict"].as_bool().unwrap_or(false), "{value}");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["severity"] == "note"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("online checks are not implemented"))),
        "{value}"
    );
}

#[test]
fn config_validate_remains_dispatchable_for_compatibility() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["config", "validate"], dir.path());

    assert!(
        !output.status.success(),
        "compatibility path should preserve strict validation status"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("config:      (none"), "{stdout}");
    assert!(stdout.contains("error: forge URL is unset"), "{stdout}");
}
