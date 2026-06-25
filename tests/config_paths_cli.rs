// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn temper(args: &[&str], env_root: &Path) -> Output {
    let xdg_config = env_root.join("xdg-config");
    let xdg_state = env_root.join("xdg-state");
    let home = env_root.join("home");
    Command::new(env!("CARGO_BIN_EXE_temper"))
        .args(args)
        .env("XDG_CONFIG_HOME", &xdg_config)
        .env("XDG_STATE_HOME", &xdg_state)
        .env("HOME", &home)
        .output()
        .expect("run temper")
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

fn toml_path(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

#[test]
fn config_paths_human_reports_default_locations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["--format", "human", "config", "paths"], dir.path());

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let config_root = dir.path().join("xdg-config").join("temper");
    let state_dir = dir.path().join("xdg-state").join("temper");

    assert!(stdout.contains("config root:"), "{stdout}");
    assert!(stdout.contains(&path_text(&config_root)), "{stdout}");
    assert!(stdout.contains("config file:"), "{stdout}");
    assert!(
        stdout.contains(&path_text(&config_root.join("config.toml"))),
        "{stdout}"
    );
    assert!(stdout.contains("credentials source:"), "{stdout}");
    assert!(
        stdout.contains(&path_text(&config_root.join("credentials.toml"))),
        "{stdout}"
    );
    assert!(stdout.contains("state dir:"), "{stdout}");
    assert!(stdout.contains(&path_text(&state_dir)), "{stdout}");
    assert!(stdout.contains("workspace dir:"), "{stdout}");
    assert!(
        stdout.contains(&path_text(&state_dir.join("workspace"))),
        "{stdout}"
    );
    assert!(stdout.contains("workflow file:"), "{stdout}");
}

#[test]
fn config_paths_json_reports_explicit_bundle_and_configured_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle");
    let workflow = dir.path().join("workflow.json");
    let workspace = dir.path().join("workspaces");
    std::fs::write(
        bundle.join("config.toml"),
        format!(
            "schema_version = 1\n\
             [engine]\n\
             workflow = \"{}\"\n\
             [worker]\n\
             workspace = \"{}\"\n",
            toml_path(&workflow),
            toml_path(&workspace),
        ),
    )
    .expect("write config");

    let bundle_arg = path_text(&bundle);
    let output = temper(
        &[
            "--format",
            "json",
            "--config",
            &bundle_arg,
            "config",
            "paths",
        ],
        dir.path(),
    );

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");

    assert_eq!(value["config_root"], path_text(&bundle));
    assert_eq!(value["config_file"], path_text(&bundle.join("config.toml")));
    assert_eq!(
        value["credentials_source"],
        path_text(&bundle.join("credentials.toml"))
    );
    assert_eq!(
        value["state_dir"],
        path_text(&dir.path().join("xdg-state").join("temper"))
    );
    assert_eq!(value["workspace_dir"], path_text(&workspace));
    assert_eq!(value["workflow_file"], path_text(&workflow));
}

#[test]
fn format_after_config_is_rejected_as_misplaced_global_option() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["config", "--format", "json", "paths"], dir.path());

    assert_eq!(output.status.code(), Some(64));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("temper config: unknown command `--format`"),
        "{stderr}"
    );
}
