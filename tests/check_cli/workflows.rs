// SPDX-License-Identifier: MPL-2.0

use serde_json::Value;

use crate::support::{WORKFLOW_JSON, temper, write_valid_bundle, write_valid_credentials};

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
         [engine]\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n",
    )
    .expect("write config");
    write_valid_credentials(&bundle);

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
fn check_json_fails_for_static_workflow_validation_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_valid_bundle(dir.path());
    std::fs::write(
        bundle.join("invalid-workflow.json"),
        r#"{
            "name": "invalid",
            "roles": [{"id": "engineer", "queues": ["missing_queue"]}]
        }"#,
    )
    .expect("write invalid workflow");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [workflow]\n\
         file = \"invalid-workflow.json\"\n\
         [paths]\n\
         state_dir = \"state\"\n\
         workspace_dir = \"workspace\"\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         admin = \"engineer\"\n\
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

    assert!(!output.status.success(), "invalid workflow should fail");
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["message"]
            .as_str()
            .is_some_and(|message| message.contains("failed validation")
                && message.contains("undeclared queue `missing_queue`"))),
        "{value}"
    );
}
