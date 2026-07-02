// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temper_scenario(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper-scenario"))
        .args(args)
        .output()
        .expect("run temper-scenario")
}

fn write_inherited_basic_delivery_bundle(bundle: &Path, name: &str) {
    std::fs::create_dir_all(bundle).expect("create inherited bundle");
    std::fs::write(
        bundle.join("scenario.toml"),
        format!(
            "name = \"{name}\"\n\
             intent = \"Ephemeral validation bundle reusing checked-in basic-delivery fixtures.\"\n\
             [fixtures]\n\
             extends = \"scenarios/basic-delivery\"\n\
             [runner]\n\
             uses = \"basic-delivery\"\n"
        ),
    )
    .expect("write inherited manifest");
}

fn read_json(path: &Path) -> serde_json::Value {
    let source = std::fs::read_to_string(path).expect("read json");
    serde_json::from_str(&source).expect("parse json")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate directory has workspace crates parent")
        .parent()
        .expect("crates directory has workspace root parent")
        .to_path_buf()
}

#[test]
fn run_writes_basic_delivery_evidence_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenario = workspace_root().join("scenarios/basic-delivery");
    let evidence = dir.path().join("basic-delivery.run-evidence.json");

    let output = temper_scenario(&[
        "run",
        "--evidence-out",
        &evidence.to_string_lossy(),
        &scenario.to_string_lossy(),
    ]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("run evidence:"), "{stdout}");
    let json = read_json(&evidence);
    assert_eq!(json["schema"], "temper.scenario.run-evidence");
    assert_eq!(json["version"], 1);
    assert_eq!(json["scenario"]["name"], "basic-delivery");
    assert_eq!(json["scenario"]["source"], "checked_in");
    assert_eq!(json["scenario"]["runner_id"], "basic-delivery");
    assert_eq!(json["scenario"]["runner_selector"], "legacy_name");
    assert_eq!(json["scenario"]["tier"], "hermetic");
    assert_eq!(
        json["scenario"]["topology"]["kind"],
        "single-repo-forgejo-standalone"
    );
    assert!(
        json["fixtures"].as_array().unwrap().iter().any(|fixture| {
            fixture["field"] == "workflow.path"
                && fixture["resolved_path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("config/workflow.json"))
        }),
        "{json:#?}"
    );
    assert_eq!(json["final_state"]["issues"][0]["state"], "closed");
    assert_eq!(json["final_state"]["pull_requests"][0]["state"], "merged");
    assert_eq!(json["final_state"]["ci"]["completed_jobs"], 1);
    assert!(json["convergence"]["ticks"].as_u64().unwrap() > 0);
}

#[test]
fn run_evidence_records_ephemeral_inherited_bundle_context() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("renamed-inherited-delivery");
    write_inherited_basic_delivery_bundle(&bundle, "renamed-inherited-delivery");
    let evidence = dir.path().join("run-evidence.json");

    let output = temper_scenario(&[
        "run",
        "--tier",
        "hermetic",
        "--evidence-out",
        &evidence.to_string_lossy(),
        &bundle.to_string_lossy(),
    ]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json = read_json(&evidence);
    assert_eq!(json["scenario"]["name"], "renamed-inherited-delivery");
    assert_eq!(json["scenario"]["source"], "ephemeral");
    assert_eq!(json["scenario"]["tier"], "hermetic");
    assert_eq!(json["scenario"]["runner_id"], "basic-delivery");
    assert_eq!(json["scenario"]["runner_selector"], "runner.uses");
    assert_eq!(
        json["scenario"]["topology"]["kind"],
        "single-repo-forgejo-standalone"
    );
    assert!(
        json["fixtures"].as_array().unwrap().iter().any(|fixture| {
            fixture["field"] == "workflow.path"
                && fixture["resolved_path"].as_str().is_some_and(|path| {
                    path.contains("scenarios/basic-delivery/config/workflow.json")
                })
        }),
        "{json:#?}"
    );
}

#[test]
fn validate_pr_ingests_run_evidence_without_rerunning_scenario() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenario = workspace_root().join("scenarios/basic-delivery");
    let evidence = dir.path().join("run-evidence.json");
    let run = temper_scenario(&[
        "run",
        "--evidence-out",
        &evidence.to_string_lossy(),
        &scenario.to_string_lossy(),
    ]);
    assert!(
        run.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let output_dir = dir.path().join("reports");

    let output = temper_scenario(&[
        "validate-pr",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--run-evidence",
        &evidence.to_string_lossy(),
        "--output-dir",
        &output_dir.to_string_lossy(),
    ]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let markdown = std::fs::read_to_string(PathBuf::from(stdout.trim())).expect("read report");
    assert!(
        markdown.contains("Run evidence artifact ingested; scenario run was not rerun"),
        "{markdown}"
    );
    assert!(
        markdown.contains(&format!("run evidence artifact: `{}`", evidence.display())),
        "{markdown}"
    );
    assert!(
        markdown.contains("scenario: `basic-delivery`"),
        "{markdown}"
    );
    assert!(
        markdown.contains("runner evidence: seeded issue:"),
        "{markdown}"
    );
    assert!(
        markdown.contains("runner evidence: report: ticks="),
        "{markdown}"
    );
    assert!(
        markdown.contains("No --scenario path was supplied with --run-evidence"),
        "{markdown}"
    );
}

#[test]
fn validate_pr_rejects_missing_and_malformed_run_evidence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing-run-evidence.json");
    let output = temper_scenario(&[
        "validate-pr",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--run-evidence",
        &missing.to_string_lossy(),
        "--output-dir",
        &dir.path().join("reports").to_string_lossy(),
    ]);
    assert!(!output.status.success(), "missing evidence should fail");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run evidence path does not exist"),
        "{stderr}"
    );

    let malformed = dir.path().join("malformed.run-evidence.json");
    std::fs::write(&malformed, "{not-json").expect("write malformed evidence");
    let output = temper_scenario(&[
        "validate-pr",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--run-evidence",
        &malformed.to_string_lossy(),
        "--output-dir",
        &dir.path().join("reports").to_string_lossy(),
    ]);
    assert!(!output.status.success(), "malformed evidence should fail");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("parse run evidence artifact"), "{stderr}");
}
