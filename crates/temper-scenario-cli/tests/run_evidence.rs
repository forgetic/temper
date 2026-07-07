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
             uses = \"basic-delivery\"\n\
             [expect]\n\
             template = \"single-pr-merged-source-closed\"\n\
             merged_pull_requests = 1\n\
             closed_parent_issues = 1\n\
             events = []\n\
             sequence = []\n\
             count = []\n\
             [[expect.checks]]\n\
             id = \"intake-triaged-and-finalized\"\n\
             artifact = \"issue:intake\"\n\
             state = \"closed\"\n\
             labels = [\"code\"]\n\
             [[expect.checks]]\n\
             id = \"implementation-pr-landed\"\n\
             artifact = \"pull_request\"\n\
             state = \"merged\"\n\
             labels_cleared = [\"landing\"]\n\
             ci = \"passed\"\n\
             [[expect.checks]]\n\
             id = \"default-branch-updated\"\n\
             artifact = \"repo:service\"\n\
             branch = \"main\"\n\
             contains_engineer_diff = true\n"
        ),
    )
    .expect("write inherited manifest");
}

fn write_failing_assertion_bundle(bundle: &Path) {
    std::fs::create_dir_all(bundle).expect("create failing assertion bundle");
    std::fs::write(
        bundle.join("scenario.toml"),
        "name = \"failing-basic-delivery-assertion\"\n\
         intent = \"Ephemeral validation bundle with an intentionally failing manifest assertion.\"\n\
         [fixtures]\n\
         extends = \"scenarios/basic-delivery\"\n\
         [runner]\n\
         uses = \"basic-delivery\"\n\
         [expect]\n\
         merged_pull_requests = 1\n\
         events = []\n\
         sequence = []\n\
         count = []\n\
         [[expect.checks]]\n\
         id = \"intentional-open-state\"\n\
         artifact = \"issue:intake\"\n\
         state = \"open\"\n",
    )
    .expect("write failing assertion manifest");
}

fn write_failing_repo_assertion_bundle(bundle: &Path) {
    std::fs::create_dir_all(bundle).expect("create failing repo assertion bundle");
    std::fs::write(
        bundle.join("scenario.toml"),
        "name = \"failing-basic-delivery-repo-assertion\"\n\
         intent = \"Ephemeral validation bundle with an intentionally failing repository assertion.\"\n\
         [fixtures]\n\
         extends = \"scenarios/basic-delivery\"\n\
         [runner]\n\
         uses = \"basic-delivery\"\n\
         [expect]\n\
         events = []\n\
         sequence = []\n\
         count = []\n\
         [[expect.checks]]\n\
         id = \"wrong-default-branch\"\n\
         artifact = \"repo:service\"\n\
         branch = \"trunk\"\n\
         contains_engineer_diff = true\n",
    )
    .expect("write failing repo assertion manifest");
}

fn read_json(path: &Path) -> serde_json::Value {
    let source = std::fs::read_to_string(path).expect("read json");
    serde_json::from_str(&source).expect("parse json")
}

#[test]
fn run_writes_basic_delivery_evidence_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenario = dir.path().join("basic-delivery-evidence");
    write_inherited_basic_delivery_bundle(&scenario, "basic-delivery-evidence");
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
    assert!(stdout.contains("assertions: passed"), "{stdout}");
    assert!(
        stdout.contains("[passed] implementation-pr-landed"),
        "{stdout}"
    );
    assert!(
        stdout.contains("[passed] default-branch-updated"),
        "{stdout}"
    );
    let json = read_json(&evidence);
    assert_eq!(json["schema"], "temper.scenario.run-evidence");
    assert_eq!(json["version"], 1);
    assert_eq!(json["scenario"]["name"], "basic-delivery-evidence");
    assert_eq!(json["scenario"]["source"], "ephemeral");
    assert_eq!(json["scenario"]["runner_id"], "basic-delivery");
    assert_eq!(json["scenario"]["runner_selector"], "runner.uses");
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
    assert_eq!(json["final_state"]["issues"][0]["id"], "intake");
    assert_eq!(json["final_state"]["issues"][0]["state"], "closed");
    assert_eq!(
        json["final_state"]["pull_requests"][0]["id"],
        "implementation"
    );
    assert_eq!(json["final_state"]["pull_requests"][0]["state"], "merged");
    assert_eq!(json["final_state"]["ci"]["completed_jobs"], 1);
    assert_eq!(
        json["final_state"]["ci"]["jobs"][0]["conclusion"],
        "success"
    );
    assert_eq!(
        json["final_state"]["ci"]["jobs"][0]["pull_request_number"],
        1
    );
    assert_eq!(json["final_state"]["repositories"][0]["id"], "service");
    assert_eq!(
        json["final_state"]["repositories"][0]["slug"],
        "acme/service"
    );
    assert_eq!(
        json["final_state"]["repositories"][0]["branches"][0]["name"],
        "main"
    );
    assert_eq!(
        json["final_state"]["repositories"][0]["branches"][0]["contains_engineer_diff"],
        true
    );
    assert_eq!(json["assertions"]["status"], "passed");
    assert_eq!(json["assertions"]["failed"], 0);
    assert_eq!(json["assertions"]["unsupported"], 0);
    assert!(
        json["assertions"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| {
                result["id"] == "implementation-pr-landed" && result["status"] == "passed"
            })
    );
    assert!(
        json["assertions"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| {
                result["id"] == "default-branch-updated" && result["status"] == "passed"
            })
    );
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
    assert_eq!(json["assertions"]["status"], "passed");
    assert_eq!(json["assertions"]["unsupported"], 0);
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
fn run_writes_failing_repo_assertion_diagnostic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("failing-repo-assertion-bundle");
    write_failing_repo_assertion_bundle(&bundle);
    let evidence = dir.path().join("failing-repo.run-evidence.json");

    let output = temper_scenario(&[
        "run",
        "--evidence-out",
        &evidence.to_string_lossy(),
        &bundle.to_string_lossy(),
    ]);

    assert!(
        !output.status.success(),
        "failing assertion should fail run"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("[failed] wrong-default-branch"), "{stdout}");
    assert!(
        stdout.contains("expected repository `service` branch `trunk` was absent"),
        "{stdout}"
    );
    assert!(stdout.contains("observed branches [\"main\"]"), "{stdout}");
    let json = read_json(&evidence);
    assert_eq!(json["assertions"]["status"], "failed");
    assert!(
        json["assertions"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| result["id"] == "wrong-default-branch" && result["status"] == "failed")
    );
}

#[test]
fn run_writes_failing_assertions_and_exits_nonzero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("failing-assertion-bundle");
    write_failing_assertion_bundle(&bundle);
    let evidence = dir.path().join("failing.run-evidence.json");

    let output = temper_scenario(&[
        "run",
        "--evidence-out",
        &evidence.to_string_lossy(),
        &bundle.to_string_lossy(),
    ]);

    assert!(
        !output.status.success(),
        "failing assertion should fail run"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("scenario: basic-delivery"), "{stdout}");
    assert!(stdout.contains("assertions: failed"), "{stdout}");
    assert!(
        stdout.contains("[failed] intentional-open-state"),
        "{stdout}"
    );
    assert!(
        stdout.contains("expected issue `intake` state `open`, observed `closed`"),
        "{stdout}"
    );
    assert!(stdout.contains("run evidence:"), "{stdout}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("manifest assertions failed"), "{stderr}");
    let json = read_json(&evidence);
    assert_eq!(json["assertions"]["status"], "failed");
    assert_eq!(json["assertions"]["failed"], 1);
    assert!(
        json["assertions"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| {
                result["id"] == "intentional-open-state" && result["status"] == "failed"
            })
    );
}

#[test]
fn validate_pr_ingests_failing_assertion_results() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("failing-assertion-bundle");
    write_failing_assertion_bundle(&bundle);
    let evidence = dir.path().join("failing.run-evidence.json");
    let run = temper_scenario(&[
        "run",
        "--evidence-out",
        &evidence.to_string_lossy(),
        &bundle.to_string_lossy(),
    ]);
    assert!(!run.status.success(), "failing assertion should fail run");
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
        !output.status.success(),
        "failing assertion report should fail"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let markdown = std::fs::read_to_string(PathBuf::from(stdout.trim())).expect("read report");
    assert!(
        markdown.contains("Manifest assertion results were ingested from run evidence"),
        "{markdown}"
    );
    assert!(
        markdown.contains("assertion failed `intentional-open-state`"),
        "{markdown}"
    );
    assert!(
        markdown.contains("expected issue `intake` state `open`"),
        "{markdown}"
    );
    assert!(markdown.contains("- Verdict: failed"), "{markdown}");
}

#[test]
fn validate_pr_ingests_run_evidence_without_rerunning_scenario() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenario = dir.path().join("basic-delivery-ingest");
    write_inherited_basic_delivery_bundle(&scenario, "basic-delivery-ingest");
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
        markdown.contains("scenario: `basic-delivery-ingest`"),
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
        markdown.contains("Manifest assertion results were ingested from run evidence"),
        "{markdown}"
    );
    assert!(
        markdown.contains("manifest assertions: passed"),
        "{markdown}"
    );
    assert!(
        markdown.contains("assertion passed `default-branch-updated`"),
        "{markdown}"
    );
    assert!(!markdown.contains("assertion unsupported"), "{markdown}");
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
