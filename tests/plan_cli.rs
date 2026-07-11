// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

#[path = "plan_cli/support.rs"]
mod support;
use support::{RecordingForge, snapshot_tree, write_bundle};

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

#[test]
fn top_level_help_lists_plan() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["--help"], dir.path());

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("\n  plan "), "{stdout}");
}

#[test]
fn plan_help_exits_successfully() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["plan", "--help"], dir.path());

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("Usage: temper [GLOBAL OPTIONS] plan"),
        "{stdout}"
    );
    assert!(stdout.contains("--existing-repo"), "{stdout}");
}

#[test]
fn one_repository_json_is_compatible_and_redacts_secrets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_bundle(
        dir.path(),
        "http://127.0.0.1:9",
        &["acme/service"],
        "",
        None,
    );
    let output = run_plan_json(&bundle, dir.path());
    let stdout = successful_stdout(&output);
    let value: Value = serde_json::from_str(&stdout).expect("plan emits JSON");

    assert_eq!(value["report_version"], 1);
    assert_eq!(value["result"], "ok");
    assert_eq!(value["repository"]["path"], "acme/service");
    assert_eq!(value["repository"], value["repositories"][0]["repository"]);
    assert_eq!(value["labels"], value["repositories"][0]["labels"]);
    assert_eq!(value["webhook"], value["repositories"][0]["webhook"]);
    assert_eq!(value["metadata"], value["repositories"][0]["metadata"]);
    assert_eq!(value["forge"]["inspected"], false);
    assert_eq!(value["webhook"]["secret"], "<redacted>");
    assert!(!stdout.contains("admin-pass"), "{stdout}");
    assert!(!stdout.contains("provider-key"), "{stdout}");
    assert!(!stdout.contains("webhook-secret-value"), "{stdout}");
}

#[test]
fn multi_repository_basic_auth_plan_is_observably_read_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let forge = RecordingForge::start();
    let bundle = write_bundle(
        dir.path(),
        forge.base_url(),
        &["acme/api", "acme/web"],
        "",
        None,
    );
    let before = snapshot_tree(&bundle);

    let output = run_plan_json(&bundle, dir.path());
    let stdout = successful_stdout(&output);
    let value: Value = serde_json::from_str(&stdout).expect("plan emits JSON");
    let after = snapshot_tree(&bundle);

    assert_eq!(before, after, "plan changed the generated pre-apply bundle");
    assert_eq!(value["repositories"].as_array().map(Vec::len), Some(2));
    assert_eq!(value["repositories"][0]["repository"]["path"], "acme/api");
    assert_eq!(value["repositories"][0]["repository"]["ci_enabled"], true);
    assert_eq!(value["repositories"][0]["webhook"]["action"], "register");
    assert_eq!(value["repositories"][1]["repository"]["path"], "acme/web");
    assert!(value.get("repository").is_none(), "{value}");
    assert!(value.get("labels").is_none(), "{value}");
    assert!(value.get("webhook").is_none(), "{value}");
    assert!(value.get("metadata").is_none(), "{value}");
    assert_eq!(value["forge"]["inspected"], true);

    let requests = forge.requests();
    assert!(!requests.is_empty());
    assert!(requests.iter().all(|request| request.method == "GET"));
    assert!(requests.iter().all(|request| {
        request
            .authorization
            .as_deref()
            .is_some_and(|authorization| authorization.starts_with("Basic "))
    }));
    assert!(
        requests
            .iter()
            .any(|request| request.path.contains("/acme/api"))
    );
    assert!(
        requests
            .iter()
            .any(|request| request.path.contains("/acme/web"))
    );
    assert!(
        requests
            .iter()
            .all(|request| !request.path.contains("/tokens")),
        "plan called a token endpoint: {requests:?}"
    );
}

#[test]
fn resolved_admin_token_is_preferred_over_basic_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let forge = RecordingForge::start();
    let bundle = write_bundle(
        dir.path(),
        forge.base_url(),
        &["acme/service"],
        "token = \"admin-token\"\n",
        None,
    );

    successful_stdout(&run_plan_json(&bundle, dir.path()));

    let requests = forge.requests();
    assert!(!requests.is_empty());
    assert!(
        requests
            .iter()
            .all(|request| { request.authorization.as_deref() == Some("token admin-token") })
    );
    assert!(requests.iter().all(|request| request.method == "GET"));
}

#[test]
fn missing_forge_credentials_report_every_repository_as_unavailable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_bundle(
        dir.path(),
        "http://127.0.0.1:9",
        &["acme/api", "acme/web"],
        "",
        None,
    );
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n[agent.providers.deepseek]\ntype = \"api-key\"\nkey = \"provider-key\"\n",
    )
    .expect("credentials without forge auth");

    let stdout = successful_stdout(&run_plan_json(&bundle, dir.path()));
    let value: Value = serde_json::from_str(&stdout).expect("plan emits JSON");

    assert_eq!(value["repositories"].as_array().map(Vec::len), Some(2));
    for repository in value["repositories"].as_array().expect("repositories") {
        assert_eq!(repository["repository"]["exists"], Value::Null);
        assert!(repository["findings"].as_array().is_some_and(|findings| {
            findings.iter().any(|finding| {
                finding["category"] == "forge"
                    && finding["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("no resolved admin token"))
            })
        }));
    }
    assert!(!stdout.contains("provider-key"), "{stdout}");
}

#[test]
fn human_plan_lists_every_repository_and_scopes_failures() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_bundle(
        dir.path(),
        "http://127.0.0.1:9",
        &["acme/api", "acme/web"],
        "",
        None,
    );
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(&["--config", &bundle_arg, "plan"], dir.path());
    let stdout = successful_stdout(&output);

    assert!(stdout.contains("Repositories (2)"), "{stdout}");
    assert!(stdout.contains("Repository acme/api"), "{stdout}");
    assert!(stdout.contains("Repository acme/web"), "{stdout}");
    assert!(stdout.contains("Labels"), "{stdout}");
    assert!(stdout.contains("Webhook"), "{stdout}");
    assert!(stdout.contains("Workflow metadata"), "{stdout}");
    assert!(stdout.contains("<redacted>"), "{stdout}");
    assert!(!stdout.contains("admin-pass"), "{stdout}");
}

#[test]
fn config_relative_json_and_yaml_workflows_load() {
    for extension in ["json", "yaml"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let workflow_name = format!("workflow.{extension}");
        let bundle = write_bundle(
            dir.path(),
            "http://127.0.0.1:9",
            &["acme/service"],
            "",
            Some(&workflow_name),
        );
        std::fs::write(bundle.join(&workflow_name), WORKFLOW_JSON).expect("workflow");

        let stdout = successful_stdout(&run_plan_json(&bundle, dir.path()));
        let value: Value = serde_json::from_str(&stdout).expect("plan emits JSON");
        assert!(
            value["workflow"]["path"]
                .as_str()
                .is_some_and(|path| path.ends_with(&workflow_name)),
            "{value}"
        );
    }
}

fn run_plan_json(bundle: &Path, env_root: &Path) -> Output {
    let bundle_arg = bundle.to_string_lossy();
    temper(
        &["--config", &bundle_arg, "--format", "json", "plan"],
        env_root,
    )
}

fn successful_stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).expect("stdout utf8")
}
