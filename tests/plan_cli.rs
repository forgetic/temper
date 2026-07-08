// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

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
fn plan_json_reports_shape_and_redacts_secrets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_plan_bundle(dir.path());
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &["--config", &bundle_arg, "--format", "json", "plan"],
        dir.path(),
    );

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("plan emits JSON");

    assert_eq!(value["result"], "ok");
    assert_eq!(value["repository"]["path"], "acme/service");
    assert_eq!(value["forge"]["inspected"], false);
    assert_eq!(value["webhook"]["secret"], "<redacted>");
    assert!(
        value["labels"]
            .as_array()
            .is_some_and(|labels| !labels.is_empty()),
        "{value}"
    );
    assert!(
        value["metadata"]["compatible"].as_bool().is_some(),
        "{value}"
    );
    assert!(!stdout.contains("admin-pass"), "{stdout}");
    assert!(!stdout.contains("provider-key"), "{stdout}");
    assert!(!stdout.contains("webhook-secret-value"), "{stdout}");
}

#[test]
fn plan_human_includes_sections_and_redacts_secret() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_plan_bundle(dir.path());
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(&["--config", &bundle_arg, "plan"], dir.path());

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");

    assert!(stdout.contains("Deployment plan:"), "{stdout}");
    assert!(stdout.contains("Repository"), "{stdout}");
    assert!(stdout.contains("Labels"), "{stdout}");
    assert!(stdout.contains("Webhook"), "{stdout}");
    assert!(stdout.contains("Workflow metadata"), "{stdout}");
    assert!(stdout.contains("<redacted>"), "{stdout}");
    assert!(!stdout.contains("admin-pass"), "{stdout}");
    assert!(!stdout.contains("provider-key"), "{stdout}");
    assert!(!stdout.contains("webhook-secret-value"), "{stdout}");
}

fn write_plan_bundle(root: &Path) -> PathBuf {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle");
    std::fs::write(bundle.join("webhook-secret"), "webhook-secret-value").expect("webhook");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [deployment]\n\
         name = \"local-dev\"\n\
         topology = \"standalone\"\n\
         [forge]\n\
         url = \"http://127.0.0.1:9\"\n\
         admin = \"root\"\n\
         ci_user = \"bot\"\n\
         [engine]\n\
         bind = \"127.0.0.1:38100\"\n\
         repos = [\"acme/service\"]\n\
         roles = [\"architect\", \"engineer\"]\n\
         webhook_secret_file = \"webhook-secret\"\n",
    )
    .expect("config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.root]\n\
         password = \"admin-pass\"\n\
         [agent.providers.deepseek]\n\
         type = \"api-key\"\n\
         key = \"provider-key\"\n",
    )
    .expect("credentials");
    bundle
}
