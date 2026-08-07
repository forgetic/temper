// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn command(current_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper-scenario"))
        .current_dir(current_dir)
        .args(args)
        .output()
        .expect("run temper-scenario")
}

#[test]
fn scaffold_writes_minimal_inherited_feature_bundle_snapshot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenarios = dir.path().join("scenarios");
    let output = command(
        &workspace_root(),
        &[
            "scaffold",
            "--feature",
            "ai/temper#778",
            "--plan",
            "ai/temper#779",
            "--source-branch",
            "feature/778-exact-head-validation",
            "--name",
            "exact-head-proof",
            "--scenarios-dir",
            &scenarios.to_string_lossy(),
        ],
    );
    assert_success(&output);
    let scenario = scenarios.join("exact-head-proof");
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout"),
        format!("{}\n", scenario.display())
    );

    let manifest = fs::read_to_string(scenario.join("scenario.toml")).expect("manifest");
    assert_eq!(
        manifest,
        "schema = \"temper.scenario.v1\"\n\
name = \"exact-head-proof\"\n\
status = \"active\"\n\
stability = \"provisional\"\n\
intent = \"Feature ai/temper#778 satisfies the contract planned by ai/temper#779.\"\n\
timeout = 600\n\
\n\
[fixtures]\n\
extends = \"scenarios/basic-delivery\"\n\
\n\
[runner]\n\
uses = \"manifest\"\n\
\n\
[jig]\n\
script_path = \"jig/exact-head-proof.json\"\n\
\n\
[validation]\n\
feature = \"ai/temper#778\"\n\
plan = \"ai/temper#779\"\n\
source_branch = \"feature/778-exact-head-validation\"\n\
change = \"new\"\n\
\n\
[feature_contract]\n\
claim = \"Feature ai/temper#778 satisfies the contract planned by ai/temper#779.\"\n\
stimulus = \"Deliver the focused feature workflow through the inherited live stack.\"\n\
observable = \"Structured Forge, CI, Temper event, and Jig request facts.\"\n\
assertion = \"Every required assertion passes at the exact feature head.\"\n\
runtime_budget_seconds = 600\n\
jig_script_path = \"jig/exact-head-proof.json\"\n"
    );
    let readme = fs::read_to_string(scenario.join("README.md")).expect("readme");
    assert!(readme.contains("Claim → stimulus → observable → assertion"));
    assert!(readme.contains("do not add credentials, generated logs, or runtime state"));
    let jig_path = scenario.join("jig/exact-head-proof.json");
    let jig: Value =
        serde_json::from_str(&fs::read_to_string(&jig_path).expect("jig")).expect("valid Jig JSON");
    assert_eq!(jig["phases"].as_array().expect("phases").len(), 2);
    jig_core::ScriptFile::load(&jig_path).expect("valid Jig script");

    let mut files = collect_files(&scenario);
    files.sort();
    assert_eq!(
        files,
        vec![
            PathBuf::from("README.md"),
            PathBuf::from("jig/exact-head-proof.json"),
            PathBuf::from("scenario.toml"),
        ]
    );
    let report = temper_scenario_core::check_scenario(&scenario);
    assert!(report.is_valid(), "{:#?}", report.diagnostics);
    let bundle = temper_testing::live_manifest::ScenarioBundle::load(&scenario)
        .expect("generated bundle is scenario-ready");
    assert_eq!(
        bundle.jig_script_path(),
        scenario.join("jig/exact-head-proof.json")
    );
}

#[test]
fn scaffold_rejects_unsafe_or_existing_output_without_overwriting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenarios = dir.path().join("scenarios");
    fs::create_dir_all(scenarios.join("proof")).expect("existing");
    fs::write(scenarios.join("proof/keep.txt"), "keep\n").expect("keep");
    let output = command(
        &workspace_root(),
        &[
            "scaffold",
            "--feature",
            "ai/temper#778",
            "--plan",
            "ai/temper#779",
            "--source-branch",
            "feature/778-exact-head-validation",
            "--name",
            "proof",
            "--scenarios-dir",
            &scenarios.to_string_lossy(),
        ],
    );
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(scenarios.join("proof/keep.txt")).expect("keep remains"),
        "keep\n"
    );

    let unsafe_output = command(
        &workspace_root(),
        &[
            "scaffold",
            "--feature",
            "ai/temper#778",
            "--plan",
            "ai/temper#779",
            "--source-branch",
            "feature/778-exact-head-validation",
            "--name",
            "../escape",
        ],
    );
    assert_eq!(unsafe_output.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&unsafe_output.stderr).contains("--name must contain only"));
}

#[test]
fn resolve_feature_emits_identical_stdout_and_ci_json() {
    let repo = tempfile::tempdir().expect("repo");
    git(repo.path(), &["init", "-q"]);
    git(
        repo.path(),
        &["config", "user.email", "tests@example.invalid"],
    );
    git(repo.path(), &["config", "user.name", "Temper Tests"]);
    fs::create_dir(repo.path().join("scenarios")).expect("scenarios");
    git(
        repo.path(),
        &["commit", "-q", "--allow-empty", "-m", "base"],
    );
    let base = git_output(repo.path(), &["rev-parse", "HEAD"]);
    write_mapped_scenario(repo.path());
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "scenario"]);

    let json_out = repo.path().join("artifacts/mapping.json");
    let output = command(
        repo.path(),
        &[
            "resolve-feature",
            "--feature",
            "ai/temper#778",
            "--landing-base",
            &base,
            "--json-out",
            &json_out.to_string_lossy(),
        ],
    );
    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert_eq!(
        stdout,
        fs::read_to_string(&json_out).expect("json artifact")
    );
    let mapping: Value = serde_json::from_str(&stdout).expect("mapping JSON");
    assert_eq!(mapping["schema"], "temper.scenario.feature-mapping.v1");
    assert_eq!(mapping["mapping_id"], "ai/temper#778:proof");
    assert_eq!(mapping["scenario_path"], "scenarios/proof");
    assert_eq!(
        mapping["source_branch"],
        "feature/778-exact-head-validation"
    );
    assert_eq!(mapping["content_changed_from_base"], true);
    assert!(
        mapping["digest"]
            .as_str()
            .expect("digest")
            .starts_with("sha256:")
    );

    let output_with_digest = command(
        repo.path(),
        &[
            "resolve-feature",
            "--feature",
            "ai/temper#778",
            "--landing-base",
            &base,
            "--expected-digest",
            mapping["digest"].as_str().expect("digest"),
        ],
    );
    assert_success(&output_with_digest);
    assert_eq!(
        serde_json::from_slice::<Value>(&output_with_digest.stdout).expect("second mapping"),
        mapping
    );
}

#[test]
fn validate_feature_rejects_stale_checkout_after_branch_advances() {
    let repo = tempfile::tempdir().expect("repo");
    git(repo.path(), &["init", "-q"]);
    git(
        repo.path(),
        &["config", "user.email", "tests@example.invalid"],
    );
    git(repo.path(), &["config", "user.name", "Temper Tests"]);
    fs::create_dir(repo.path().join("scenarios")).expect("scenarios");
    git(
        repo.path(),
        &["commit", "-q", "--allow-empty", "-m", "base"],
    );
    let base = git_output(repo.path(), &["rev-parse", "HEAD"]);
    write_mapped_scenario(repo.path());
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "scenario"]);
    let stale_head = git_output(repo.path(), &["rev-parse", "HEAD"]);
    git(
        repo.path(),
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "advance feature branch",
        ],
    );
    let landing_pr_head = git_output(repo.path(), &["rev-parse", "HEAD"]);
    git(repo.path(), &["checkout", "-q", "--detach", &stale_head]);

    let output_dir = repo.path().join("artifacts/focused");
    let output = command(
        repo.path(),
        &[
            "validate-feature",
            "--feature",
            "ai/temper#778",
            "--landing-base",
            &base,
            "--source-branch",
            "feature/778-exact-head-validation",
            "--pr",
            "42",
            "--sha",
            &landing_pr_head,
            "--output-dir",
            &output_dir.to_string_lossy(),
        ],
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "checked-out HEAD `{stale_head}` does not match supplied landing PR head `{landing_pr_head}`"
        )),
        "{stderr}"
    );
    let mapping: Value = serde_json::from_slice(
        &fs::read(output_dir.join("feature-scenario-mapping.json")).expect("mapping artifact"),
    )
    .expect("mapping JSON");
    assert_eq!(mapping["scenario_path"], "scenarios/proof");
    assert_eq!(mapping["head_sha"], stale_head);
    assert!(output_dir.join("focused-validation-failure.txt").is_file());
    assert!(!output_dir.join("run-evidence.json").exists());
}

#[test]
fn validate_feature_runs_resolved_scenario_and_retains_failed_evidence() {
    let repo = tempfile::tempdir().expect("repo");
    git(repo.path(), &["init", "-q"]);
    git(
        repo.path(),
        &["config", "user.email", "tests@example.invalid"],
    );
    git(repo.path(), &["config", "user.name", "Temper Tests"]);
    fs::create_dir(repo.path().join("scenarios")).expect("scenarios");
    git(
        repo.path(),
        &["commit", "-q", "--allow-empty", "-m", "base"],
    );
    let base = git_output(repo.path(), &["rev-parse", "HEAD"]);
    write_mapped_scenario(repo.path());
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "scenario"]);
    let head = git_output(repo.path(), &["rev-parse", "HEAD"]);
    let output_dir = repo.path().join("artifacts/focused");

    let output = command(
        repo.path(),
        &[
            "validate-feature",
            "--feature",
            "ai/temper#778",
            "--landing-base",
            &base,
            "--source-branch",
            "feature/778-exact-head-validation",
            "--pr",
            "42",
            "--sha",
            &head,
            "--output-dir",
            &output_dir.to_string_lossy(),
            "--temper-bin",
            &repo.path().join("missing-temper").to_string_lossy(),
        ],
    );

    assert!(!output.status.success());
    let audit: Value = serde_json::from_slice(
        &fs::read(output_dir.join("focused-validation-audit.json")).expect("focused audit"),
    )
    .expect("audit JSON");
    assert_eq!(audit["status"], "failed");
    assert_eq!(audit["landing_pr_head_sha"], head);
    assert_eq!(audit["mapping"]["scenario_path"], "scenarios/proof");
    assert_eq!(
        audit["validator_result"]["mapping_id"],
        "ai/temper#778:proof"
    );
    assert_eq!(audit["validator_result"]["exact_head_sha"], head);
    assert_eq!(audit["validator_result"]["verdict"], "failed");
    assert!(output_dir.join("run-evidence.json").is_file());
}

#[test]
fn validate_feature_retains_failure_when_mapping_is_missing() {
    let repo = tempfile::tempdir().expect("repo");
    git(repo.path(), &["init", "-q"]);
    git(
        repo.path(),
        &["config", "user.email", "tests@example.invalid"],
    );
    git(repo.path(), &["config", "user.name", "Temper Tests"]);
    fs::create_dir(repo.path().join("scenarios")).expect("scenarios");
    git(
        repo.path(),
        &["commit", "-q", "--allow-empty", "-m", "base"],
    );
    let head = git_output(repo.path(), &["rev-parse", "HEAD"]);
    let output_dir = repo.path().join("artifacts/focused");

    let output = command(
        repo.path(),
        &[
            "validate-feature",
            "--feature",
            "ai/temper#778",
            "--landing-base",
            &head,
            "--source-branch",
            "feature/778-exact-head-validation",
            "--pr",
            "42",
            "--sha",
            &head,
            "--output-dir",
            &output_dir.to_string_lossy(),
        ],
    );

    assert!(!output.status.success());
    let failure = fs::read_to_string(output_dir.join("focused-validation-failure.txt"))
        .expect("retained mapping failure");
    assert!(
        failure.contains("no scenario explicitly maps feature"),
        "{failure}"
    );
}

fn write_mapped_scenario(root: &Path) {
    let scenario = root.join("scenarios/proof");
    fs::create_dir_all(scenario.join("jig")).expect("scenario");
    fs::write(scenario.join("jig/proof.json"), "{}\n").expect("jig");
    fs::write(
        scenario.join("scenario.toml"),
        "schema = \"temper.scenario.v1\"\n\
name = \"proof\"\n\
status = \"active\"\n\
stability = \"provisional\"\n\
intent = \"Prove feature mapping.\"\n\
[runner]\n\
uses = \"manifest\"\n\
[validation]\n\
feature = \"ai/temper#778\"\n\
plan = \"ai/temper#779\"\n\
source_branch = \"feature/778-exact-head-validation\"\n\
change = \"new\"\n\
[feature_contract]\n\
claim = \"Feature works.\"\n\
stimulus = \"Run it.\"\n\
observable = \"Structured facts.\"\n\
assertion = \"Facts pass.\"\n\
runtime_budget_seconds = 600\n\
jig_script_path = \"jig/proof.json\"\n",
    )
    .expect("manifest");
}

fn collect_files(root: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path).expect("read dir") {
            let entry = entry.expect("entry");
            if entry.path().is_dir() {
                pending.push(entry.path());
            } else {
                result.push(
                    entry
                        .path()
                        .strip_prefix(root)
                        .expect("relative")
                        .to_path_buf(),
                );
            }
        }
    }
    result
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_string()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates")
        .parent()
        .expect("workspace")
        .to_path_buf()
}
